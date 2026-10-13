#!/bin/bash
# Run a command up to 5 times, 30 s apart. For network steps that fail on a dropped connection, such as
# stapler's CloudKit ticket lookup (NSURLErrorDomain -1005, "The network connection was lost").
#
#   packaging/retry.sh xcrun stapler staple dist/release/Taskboard.app
set -uo pipefail
for try in 1 2 3 4 5; do
  "$@" && exit 0
  [ "$try" = 5 ] && break
  echo "retry.sh: try $try of 5 failed: $*; trying again in 30 s" >&2
  sleep 30
done
exit 1
