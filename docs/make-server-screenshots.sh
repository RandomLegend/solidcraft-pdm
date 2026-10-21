#!/usr/bin/env bash
# Makes the server's README images in docs/images/ (server-dashboard.png, server-users.png): a
# demo server with users, vaults and a month of check-ins, made with the real command line,
# photographed in headless Chromium by docs/server-screenshots.js.
#
#   CHROMIUM=/path/to/chrome docs/make-server-screenshots.sh
#
# Needs Node.js (playwright-core is installed into target/), Python 3, and a Chromium or Chrome
# (CHROMIUM; by default the one Playwright downloads to ~/.cache/ms-playwright). The app's
# picture on a server (server-app.png) comes from docs/make-screenshots.sh.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
chromium="${CHROMIUM:-$(ls -d "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -n1)}"
[ -x "$chromium" ] || { echo "set CHROMIUM to a Chromium or Chrome program" >&2; exit 1; }
node_dir="$root/target/readme/node"
[ -d "$node_dir/node_modules/playwright-core" ] || npm install --silent --prefix "$node_dir" playwright-core@1.56.1

cargo build --release --manifest-path "$root/Cargo.toml" -p scpdm-server -p scpdm
server="$root/target/release/scpdm-server"
cli="$root/target/release/scpdm"
work="$(mktemp -d)"
export SCPDM_SERVER_DATA="$work/pdm-data"
port=7389
url="http://127.0.0.1:$port"
trap 'kill "$pid" 2>/dev/null || true; rm -rf "$work"' EXIT

# Users and vaults.
echo "admin-password" | "$server" user add admin --admin --password-stdin
for user in anna ben carla dana; do echo "$user-password" | "$server" user add "$user" --password-stdin; done
for vault in workshop office prototypes; do "$server" vault add "$vault"; done
"$server" grant anna workshop write
"$server" grant ben workshop write
"$server" grant ben office write
"$server" grant carla office read
"$server" grant anna prototypes write
"$server" grant dana prototypes write
"$server" serve --listen "127.0.0.1:$port" --name "Workshop server" >"$work/server.log" 2>&1 &
pid=$!
sleep 1

# Designs and their history, as the team would make them (from the test fixtures, renamed).
fixtures="$root/crates/scformat/tests/fixtures"
variant() { # fixture, design file, name, iteration
  python3 -I -c 'import json, sys
d = json.load(open(sys.argv[1])); d["name"] = sys.argv[3]
d.setdefault("metadata", {})["description"] = f"{sys.argv[3]}, iteration {sys.argv[4]}"
json.dump(d, open(sys.argv[2], "w"), indent=1)' "$fixtures/$1" "$2" "$3" "$4"
}
join() { SCPDM_PASSWORD="$1-password" "$cli" remote join "$url" "$1" "$2" "$work/$1-$2" >/dev/null; }
add() { # workspace, path, fixture, name, comment
  (cd "$work/$1" && mkdir -p "$(dirname "$2")" && variant "$3" "$2" "$4" 1 && "$cli" remote add "$2" -m "$5" >/dev/null)
}
change() { # workspace, path, fixture, name, iteration, comment
  (cd "$work/$1" && { "$cli" remote get "$2" >/dev/null 2>&1 || true; } && "$cli" remote checkout "$2" >/dev/null \
    && variant "$3" "$2" "$4" "$5" && "$cli" remote ci "$2" -m "$6" >/dev/null)
}
join anna workshop; join ben workshop; join anna prototypes; join dana prototypes; join ben office
add anna-workshop Enclosures/pi4-case.solidcraft json-pi-case.solidcraft pi4-case "First case"
add anna-workshop Enclosures/pi4-lid.solidcraft json-pi-case.solidcraft pi4-lid "Lid with vents"
add ben-workshop Hardware/wall-bracket.solidcraft json-move-bodies.solidcraft wall-bracket "Bracket, 2 mm sheet"
add ben-workshop Hardware/hinge.solidcraft json-move-bodies.solidcraft hinge "40 mm butt hinge"
add anna-workshop Assemblies/case-assembly.solidcraft json-components.solidcraft case-assembly "Case with lid"
change anna-workshop Enclosures/pi4-case.solidcraft json-pi-case.solidcraft pi4-case 2 "Port cut-outs"
change anna-workshop Enclosures/pi4-case.solidcraft json-pi-case.solidcraft pi4-case 3 "Wider board pocket"
change ben-workshop Hardware/wall-bracket.solidcraft json-move-bodies.solidcraft wall-bracket 2 "Countersunk holes"
change ben-workshop Enclosures/pi4-lid.solidcraft json-pi-case.solidcraft pi4-lid 2 "Thicker ribs"
change anna-workshop Assemblies/case-assembly.solidcraft json-components.solidcraft case-assembly 2 "Screws added"
(cd "$work/anna-workshop" && "$cli" remote release Enclosures/pi4-case.solidcraft --revision A -m "For the first batch" >/dev/null)
add anna-prototypes Drone/frame.solidcraft json-move-bodies.solidcraft drone-frame "Frame, 5 inch"
add dana-prototypes Drone/motor-mount.solidcraft json-move-bodies.solidcraft motor-mount "Motor mount"
change dana-prototypes Drone/motor-mount.solidcraft json-move-bodies.solidcraft motor-mount 2 "Lighter"
change dana-prototypes Drone/motor-mount.solidcraft json-move-bodies.solidcraft motor-mount 3 "Back to 3 mm"
add ben-office Fixtures/desk-clamp.solidcraft json-pi-case.solidcraft desk-clamp "Clamp for the 25 mm desk"
(cd "$work/ben-workshop" && "$cli" remote get --all >/dev/null && "$cli" remote checkout Assemblies/case-assembly.solidcraft >/dev/null)
(cd "$work/dana-prototypes" && "$cli" remote get --all >/dev/null && "$cli" remote checkout Drone/frame.solidcraft >/dev/null)

# A month of history instead of a minute: the versions spread over four weeks (in order).
kill "$pid"; wait "$pid" 2>/dev/null || true
python3 -I - "$SCPDM_SERVER_DATA/vaults" <<'EOF'
import os, random, sqlite3, sys, time
random.seed(7)
now = int(time.time())
for vault in sorted(os.listdir(sys.argv[1])):
    db = sqlite3.connect(os.path.join(sys.argv[1], vault, ".pdm", "vault.db"))
    ids = [r[0] for r in db.execute("SELECT id FROM versions ORDER BY id")]
    times = sorted(min(now - 600, now - random.randint(0, 27) * 86400 - random.randint(0, 9) * 3600) for _ in ids)
    for vid, t in zip(ids, times):
        db.execute("UPDATE versions SET created_at = ? WHERE id = ?", (t, vid))
    db.execute("UPDATE releases SET released_at = (SELECT MAX(created_at) FROM versions)")
    db.execute("UPDATE locks SET since = ?", (now - random.randint(2, 20) * 3600,))
    db.commit()
EOF
"$server" serve --listen "127.0.0.1:$port" --name "Workshop server" >"$work/server.log" 2>&1 &
pid=$!
sleep 1
# The users' logins as the app would have them (for "online").
for user in anna ben dana; do
  curl -fsS -o /dev/null -H 'Content-Type: application/json' -d "{\"user\":\"$user\",\"password\":\"$user-password\"}" "$url/api/login"
done

mkdir -p "$root/docs/images"
NODE_PATH="$node_dir/node_modules" node "$root/docs/server-screenshots.js" "$chromium" "$port" "$root/docs/images"
ls -la "$root"/docs/images/server-*.png
