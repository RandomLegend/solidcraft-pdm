#!/bin/sh
# scpdm-server in the image: runs the server program as PUID:PGID (by default 7380, the image's
# user "pdm"). Set them to the owner of the backup folder (a share, say) so the server may write
# there. Commands given with `docker compose exec` go through here too, so files they make in
# /data belong to the same user.
set -eu
uid="${PUID:-7380}"
gid="${PGID:-$uid}"
program=/usr/local/lib/scpdm/scpdm-server
if [ "$(id -u)" != 0 ]; then
    exec "$program" "$@"
fi
# /data belongs to the server's user (a new volume, or PUID changed); /backups is left as it is:
# a share may not allow changing owners, and the error says what to do.
if [ "$(stat -c %u /data)" != "$uid" ]; then
    chown -R "$uid:$gid" /data
fi
exec setpriv --reuid="$uid" --regid="$gid" --clear-groups "$program" "$@"
