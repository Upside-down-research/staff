#!/bin/bash -e
set -o pipefail
which protoc-gen-prost > /dev/null
protoc --prost_out=src  protos/staff.proto
