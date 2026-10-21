//! The administration page and its API, against a running server.

use protocol::Action;
use scpdm_server::Server;
use serde_json::{Value, json};

const DESIGN: &[u8] = br#"{"format": "solidcraft/29", "name": "lid", "features": []}"#;

struct Setup {
    _data: tempfile::TempDir,
    url: String,
    agent: ureq::Agent,
}

/// A server with an administrator (root), a user (ben, may change "workshop") and a design
/// in "workshop" that ben has checked out.
fn setup() -> Setup {
    let data = tempfile::tempdir().unwrap();
    let server = Server::open(data.path(), "Workshop").unwrap();
    server.add_vault("workshop").unwrap();
    server.accounts.add_user("root", "root-password", true).unwrap();
    server.accounts.add_user("ben", "ben-password", false).unwrap();
    server.accounts.grant("ben", "workshop", protocol::Access::Write).unwrap();
    let url = scpdm_server::spawn_for_tests(server);
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let s = Setup { _data: data, url, agent };

    let ben = s.app_login("ben", "ben-password");
    let hash = vault::hash(DESIGN);
    let put = s.agent.put(&format!("{}/api/vaults/workshop/objects/{hash}", s.url)).header("Authorization", &format!("Bearer {ben}")).send(DESIGN).unwrap();
    assert_eq!(put.status(), 201);
    for action in [Action::Add { path: "parts/lid.solidcraft".into(), hash, comment: "First lid".into() }, Action::Checkout { path: "parts/lid.solidcraft".into() }] {
        let name = serde_json::to_value(&action).unwrap()["action"].as_str().unwrap().to_owned();
        let response = s
            .agent
            .post(&format!("{}/api/vaults/workshop/{name}", s.url))
            .header("Authorization", &format!("Bearer {ben}"))
            .send_json(&action)
            .unwrap();
        assert_eq!(response.status(), 200, "{name}");
    }
    s
}

impl Setup {
    /// A login as the app makes it: the token.
    fn app_login(&self, user: &str, password: &str) -> String {
        let mut r = self.agent.post(&format!("{}/api/login", self.url)).send_json(json!({"user": user, "password": password})).unwrap();
        assert_eq!(r.status(), 200);
        r.body_mut().read_json::<Value>().unwrap()["token"].as_str().unwrap().to_owned()
    }

    /// Logs in on the page: the cookie it sets.
    fn page_login(&self, user: &str, password: &str) -> (u16, Option<String>) {
        let r = self
            .agent
            .post(&format!("{}/api/admin/login", self.url))
            .header("X-PDM-Admin", "1")
            .send_json(json!({"user": user, "password": password}))
            .unwrap();
        let cookie = r.headers().get("set-cookie").map(|v| v.to_str().unwrap().to_owned());
        (r.status().as_u16(), cookie)
    }

    fn get(&self, cookie: &str, path: &str) -> (u16, Value) {
        let mut r = self.agent.get(&format!("{}{path}", self.url)).header("Cookie", cookie).call().unwrap();
        let status = r.status().as_u16();
        (status, r.body_mut().read_json().unwrap_or(Value::Null))
    }

    /// A change as the page sends it (with the change header, unless `header` is false).
    fn send(&self, cookie: &str, method: &str, path: &str, body: Value, header: bool) -> u16 {
        let url = format!("{}{path}", self.url);
        let request = match method {
            "POST" => self.agent.post(&url),
            "PUT" => self.agent.put(&url),
            _ => unreachable!(),
        };
        let request = request.header("Cookie", cookie);
        let request = if header { request.header("X-PDM-Admin", "1") } else { request };
        request.send_json(body).unwrap().status().as_u16()
    }

    fn delete(&self, cookie: &str, path: &str) -> u16 {
        let r = self.agent.delete(&format!("{}{path}", self.url)).header("Cookie", cookie).header("X-PDM-Admin", "1").call().unwrap();
        r.status().as_u16()
    }
}

/// `pdm_admin=…` from a Set-Cookie header.
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_owned()
}

#[test]
fn the_page_is_served_and_kept_to_itself() {
    let s = setup();
    let mut r = s.agent.get(&format!("{}/", s.url)).call().unwrap();
    assert_eq!(r.status(), 200, "/ leads to the page");
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_owned();
    assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(r.body_mut().read_to_string().unwrap().contains("app.js"));
    for file in ["app.js", "app.css", "icon.svg"] {
        assert_eq!(s.agent.get(&format!("{}/admin/{file}", s.url)).call().unwrap().status(), 200, "{file}");
    }
}

#[test]
fn only_administrators_log_in() {
    let s = setup();
    assert_eq!(s.page_login("root", "wrong").0, 401);
    let (status, cookie) = s.page_login("ben", "ben-password");
    assert_eq!((status, cookie), (403, None), "ben is no administrator");
    let no_header = s.agent.post(&format!("{}/api/admin/login", s.url)).send_json(json!({"user": "root", "password": "root-password"})).unwrap();
    assert_eq!(no_header.status(), 403, "another site's form cannot log in");

    let (status, cookie) = s.page_login("root", "root-password");
    assert_eq!(status, 200);
    let cookie = cookie.unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    let (status, me) = s.get(&cookie_pair(&cookie), "/api/admin/me");
    assert_eq!((status, me["user"].as_str()), (200, Some("root")));

    // The app's login works too, but not a user's.
    let ben = s.app_login("ben", "ben-password");
    let r = s.agent.get(&format!("{}/api/admin/overview", s.url)).header("Authorization", &format!("Bearer {ben}")).call().unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(s.get("pdm_admin=forged", "/api/admin/overview").0, 401);
}

#[test]
fn the_dashboard_counts_and_shows_check_outs() {
    let s = setup();
    let cookie = cookie_pair(&s.page_login("root", "root-password").1.unwrap());
    let (status, o) = s.get(&cookie, "/api/admin/overview");
    assert_eq!(status, 200, "{o}");
    let t = &o["totals"];
    assert_eq!((t["users"].as_u64(), t["admins"].as_u64(), t["vaults"].as_u64()), (Some(2), Some(1), Some(1)));
    assert_eq!((t["designs"].as_u64(), t["versions"].as_u64(), t["checked_out"].as_u64()), (Some(1), Some(1), Some(1)));
    assert_eq!(t["online"].as_u64(), Some(2), "both logged in just now");
    assert_eq!(t["stored_bytes"].as_u64(), Some(DESIGN.len() as u64));
    assert_eq!(o["per_day"].as_array().unwrap().len(), 30);
    assert_eq!(o["per_day"][29][1].as_u64(), Some(1), "today's check-in");
    assert_eq!(o["recent"][0]["author"].as_str(), Some("ben"));
    assert_eq!(o["checked_out"][0]["user"].as_str(), Some("ben"));

    // Breaking ben's check-out.
    let unlock = json!({"path": "parts/lid.solidcraft"});
    assert_eq!(s.send(&cookie, "POST", "/api/admin/vaults/workshop/unlock", unlock.clone(), false), 403, "needs the header");
    assert_eq!(s.send(&cookie, "POST", "/api/admin/vaults/workshop/unlock", unlock, true), 204);
    assert_eq!(s.get(&cookie, "/api/admin/overview").1["totals"]["checked_out"].as_u64(), Some(0));

    let (_, vaults) = s.get(&cookie, "/api/admin/vaults");
    assert_eq!(vaults[0]["name"].as_str(), Some("workshop"));
    assert_eq!((vaults[0]["designs"].as_u64(), vaults[0]["users"].as_u64()), (Some(1), Some(1)));
}

#[test]
fn users_and_their_access_are_managed() {
    let s = setup();
    let cookie = cookie_pair(&s.page_login("root", "root-password").1.unwrap());
    let new = json!({"name": "carla", "password": "carla-password"});
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users", new.clone(), false), 403, "needs the header");
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users", new.clone(), true), 201);
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users", new, true), 400, "exists already");
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users", json!({"name": "d", "password": "short"}), true), 400);

    assert_eq!(s.send(&cookie, "PUT", "/api/admin/users/carla/access/workshop", json!({"access": "read"}), true), 204);
    assert_eq!(s.send(&cookie, "PUT", "/api/admin/users/carla/access/nowhere", json!({"access": "read"}), true), 404);
    assert_eq!(s.send(&cookie, "PUT", "/api/admin/users/carla/access/workshop", json!({"access": "everything"}), true), 400);
    let (_, users) = s.get(&cookie, "/api/admin/users");
    let carla = users.as_array().unwrap().iter().find(|u| u["name"] == "carla").unwrap().clone();
    assert_eq!(carla["access"], json!({"workshop": "read"}));
    assert_eq!((carla["admin"].as_bool(), carla["sessions"].as_u64()), (Some(false), Some(0)));

    // Carla's logins end with a new password, or when told to.
    let token = s.app_login("carla", "carla-password");
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users/carla", json!({"end_sessions": true}), true), 204);
    let r = s.agent.get(&format!("{}/api/vaults", s.url)).header("Authorization", &format!("Bearer {token}")).call().unwrap();
    assert_eq!(r.status(), 401, "logged out");
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users/carla", json!({"password": "new-password", "admin": true}), true), 204);
    s.app_login("carla", "new-password");
    assert_eq!(s.page_login("carla", "new-password").0, 200, "now an administrator");

    // Not oneself.
    assert_eq!(s.send(&cookie, "POST", "/api/admin/users/root", json!({"admin": false}), true), 409);
    assert_eq!(s.delete(&cookie, "/api/admin/users/root"), 409);
    assert_eq!(s.delete(&cookie, "/api/admin/users/carla"), 204);
    assert_eq!(s.delete(&cookie, "/api/admin/users/carla"), 400, "gone");

    assert_eq!(s.send(&cookie, "POST", "/api/admin/vaults", json!({"name": "office"}), true), 201);
    assert_eq!(s.send(&cookie, "POST", "/api/admin/vaults", json!({"name": "../x"}), true), 400);
    let (_, vaults) = s.get(&cookie, "/api/admin/vaults");
    assert_eq!(vaults.as_array().unwrap().len(), 2);

    // Logging out ends the page's login.
    assert_eq!(s.send(&cookie, "POST", "/api/admin/logout", json!({}), true), 204);
    assert_eq!(s.get(&cookie, "/api/admin/me").0, 401);
}
