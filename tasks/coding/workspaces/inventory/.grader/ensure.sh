#!/bin/sh
# Make sure step N ($1) is in place before the next step starts: if the
# hidden tests of step N fail (the earlier step was not solved, or this is a
# resumed run with a fresh workspace), install the reference solution for
# that step so the next step is measured on its own work.
k="${1:-1}"
if sh .grader/run.sh "$k" > /dev/null 2>&1; then
  exit 0
fi
cp -R ".grader/ref/step$k/." .
exit 0
