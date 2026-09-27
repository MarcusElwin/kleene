#!/bin/sh
# Grader: run the hidden tests of steps 1..N (N = $1) against the workspace.
# Exit 0 only if every step's tests pass. Test files are the grader's own
# copies, so edits under tests/ do not change the verdict.
k="${1:-1}"
rc=0
summary=""
i=1
while [ "$i" -le "$k" ]; do
  out=$(python3 .grader/tests/test_step$i.py 2>&1)
  r=$?
  last=$(printf '%s\n' "$out" | tail -n 1)
  summary="$summary step$i=$last"
  [ "$r" -ne 0 ] && rc=1
  i=$((i + 1))
done
echo "$summary"
if [ "$rc" -ne 0 ]; then
  printf '%s\n' "$out" | tail -n 25
fi
exit $rc
