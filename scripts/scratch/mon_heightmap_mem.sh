#!/usr/bin/env bash
# Monitor subprocess peak RSS (physical memory, MB), no virtual limit set
"$@" > assets/heightmaps/hm_out.log 2>&1 &
PID=$!
PEAK=0
while kill -0 $PID 2>/dev/null; do
  V=$(grep VmRSS /proc/$PID/status 2>/dev/null | awk '{print $2}')
  if [ -n "$V" ] && [ "$V" -gt "$PEAK" ]; then PEAK=$V; fi
  sleep 0.2
done
wait $PID; RC=$?
echo "PEAK RSS: $((PEAK/1024)) MB, exit=$RC"
tail -3 assets/heightmaps/hm_out.log
exit 0
