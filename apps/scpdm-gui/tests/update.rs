//! Looking for a new version from the settings, against a stand-in for GitLab's API.

use std::io::{Read, Write};
use std::net::TcpListener;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use scpdm_gui::app::App;

/// Answers every request with a release v9.0.0 (whose files are not needed here).
fn gitlab() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let release = serde_json::json!({
        "tag_name": "v9.0.0",
        "description": "### Faster\n\n- Everything is quicker",
        "_links": {"self": "https://example.org/release"},
        "assets": {"links": []},
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                request.push(byte[0]);
            }
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", release.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(release.as_bytes());
        }
    });
    url
}

fn wait_for(h: &mut Harness<'_, App>, label: &str) {
    for _ in 0..400 {
        h.step();
        if h.query_by_label_contains(label).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{label} never showed");
}

#[test]
fn points_out_a_newer_release() {
    // The only test in this binary: nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("SCPDM_UPDATE_API", gitlab()) };
    let mut h = Harness::builder().with_size([1280.0, 800.0]).with_step_dt(1.0 / 60.0).build_eframe(|cc| App::new(cc, None));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Update to").is_none(), "a build from source does not look by itself");
    h.get_by_label("Vault").click();
    h.run_steps(2);
    h.get_by_label("Settings…").click();
    h.run_steps(3);
    h.get_by_label("Updates").click();
    h.run_steps(3);
    h.get_by_label("Check now").click();
    wait_for(&mut h, "SolidCraft PDM 9.0.0");
    h.run_steps(2);
    h.get_by_label("Faster");
    h.get_by_label("- Everything is quicker");
    h.get_by_label_contains("not installed from a release");
    assert!(h.query_by_label("Install and restart").is_none(), "nothing to install from here");
    h.get_by_label("Later").click();
    h.run_steps(3);

    h.get_by_label("⬆ Update to 9.0.0").click();
    h.run_steps(3);
    h.get_by_label("Skip this version").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Update to").is_none());
}
