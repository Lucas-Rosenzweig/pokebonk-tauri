#!/usr/bin/env bash
# Starts the Linux packages on a virtual screen, keeps each self-test report, its log and two screenshots.
set -u
fail=0
mkdir -p smoke
Xvfb :99 -screen 0 1600x900x24 >/dev/null 2>&1 &
export DISPLAY=:99
sleep 2

run() { # name, command...
  local name=$1
  shift
  mkdir -p "smoke/$name"
  dbus-run-session -- "$@" --selftest="$PWD/smoke/$name" --selftest-wait=45000 >"smoke/$name/log.txt" 2>&1 &
  local pid=$!
  sleep 20
  import -display :99 -window root "smoke/$name/20s.png" || true
  sleep 18
  import -display :99 -window root "smoke/$name/38s.png" || true
  wait "$pid"
  echo "$name exit=$?" | tee -a smoke/summary.txt
  if [ -f "smoke/$name/report.json" ]; then
    cat "smoke/$name/report.json"
  else
    echo "$name: no report" | tee -a smoke/summary.txt
    fail=1
  fi
}

ls -la src-tauri/target/release/bundle/*/ | tee smoke/bundles.txt
sudo apt-get install -y ./src-tauri/target/release/bundle/deb/*.deb
dpkg -s poke-bonk-tauri 2>/dev/null | tee smoke/deb-status.txt || dpkg -l | grep -i bonk | tee smoke/deb-status.txt
dpkg -L "$(dpkg -l | awk '/bonk/ {print $2; exit}')" | tee smoke/deb-files.txt
run deb /usr/bin/pokebonk-tauri

appimage=$(ls src-tauri/target/release/bundle/appimage/*.AppImage | head -1)
chmod +x "$appimage"
export APPIMAGE_EXTRACT_AND_RUN=1
run appimage "$appimage"

exit $fail
