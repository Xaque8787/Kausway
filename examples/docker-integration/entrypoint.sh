#!/bin/sh
# CMD / docker run arguments are passed intact to the lifecycle supervisor.
exec python3 /opt/kausway-example/supervise.py "$@"
