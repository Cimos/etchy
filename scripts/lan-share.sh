#!/usr/bin/env bash
# Share the etchy review site on the private network (WSL2).
#
#   scripts/lan-share.sh up      start the site+feedback server and add the Windows
#                                portproxy + firewall rule (prompts Windows UAC)
#   scripts/lan-share.sh down    stop the server and remove the portproxy + firewall rule
#   scripts/lan-share.sh check   print what 'up' would do (no changes, no UAC)
#
# Why the Windows step: the server runs inside WSL2, which has its own NAT'd IP, so
# Windows must forward the LAN port to the WSL IP. The WSL IP can change on reboot —
# just run 'up' again to refresh it.
set -euo pipefail
PORT="${FEEDBACK_PORT:-8000}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WSLIP="$(hostname -I | awk '{print $1}')"
WINPS1='C:\Users\simad\etchy-lan-share.ps1'
WINPS1_WSL='/mnt/c/Users/simad/etchy-lan-share.ps1'
cmd="${1:-up}"

win_ip() {
  powershell.exe -NoProfile -Command \
    "(Get-NetIPAddress -AddressFamily IPv4 | Where-Object {\$_.IPAddress -like '192.168.*' -or \$_.IPAddress -like '10.*'} | Select-Object -First 1).IPAddress" \
    2>/dev/null | tr -d '\r ' || true
}
run_elevated() { # $1 = ps1 body
  printf '%s\n' "$1" > "$WINPS1_WSL"
  powershell.exe -NoProfile -Command \
    "Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$WINPS1'" >/dev/null 2>&1 || true
}

UP_PS1="netsh interface portproxy delete v4tov4 listenport=$PORT listenaddress=0.0.0.0 2>\$null
netsh interface portproxy add v4tov4 listenport=$PORT listenaddress=0.0.0.0 connectport=$PORT connectaddress=$WSLIP
if (-not (Get-NetFirewallRule -DisplayName 'etchy site $PORT' -ErrorAction SilentlyContinue)) { New-NetFirewallRule -DisplayName 'etchy site $PORT' -Direction Inbound -LocalPort $PORT -Protocol TCP -Action Allow | Out-Null }
Write-Host 'etchy LAN share ready on port $PORT (WSL $WSLIP)'
Start-Sleep 2"

DOWN_PS1="netsh interface portproxy delete v4tov4 listenport=$PORT listenaddress=0.0.0.0 2>\$null
Remove-NetFirewallRule -DisplayName 'etchy site $PORT' -ErrorAction SilentlyContinue
Write-Host 'etchy LAN share removed'
Start-Sleep 2"

case "$cmd" in
  up)
    if ! curl -fsS "http://127.0.0.1:$PORT/" >/dev/null 2>&1; then
      nohup python3 "$HERE/site-server.py" >/tmp/etchy-site-server.log 2>&1 &
      for _ in 1 2 3 4 5 6 7 8; do curl -fsS "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.5; done
      echo "started site server (log: /tmp/etchy-site-server.log)"
    else
      echo "site server already running on :$PORT"
    fi
    run_elevated "$UP_PS1"
    echo "Applying Windows portproxy + firewall — approve the UAC prompt."
    echo "Teammates open:  http://$(win_ip):$PORT/"
    echo "(you: http://localhost:$PORT/)"
    ;;
  down)
    pkill -f "site-server.py" 2>/dev/null || true
    run_elevated "$DOWN_PS1"
    echo "Stopped server; removing Windows portproxy + firewall — approve the UAC prompt."
    ;;
  check)
    echo "PORT=$PORT  WSL IP=$WSLIP  Windows LAN IP=$(win_ip)"
    echo "Teammates would use:  http://$(win_ip):$PORT/"
    echo "--- Windows commands 'up' runs (elevated) ---"; echo "$UP_PS1"
    ;;
  *) echo "usage: lan-share.sh [up|down|check]" >&2; exit 2 ;;
esac
