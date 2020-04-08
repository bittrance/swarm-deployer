#!/bin/bash

set -e

[ -n "$1" ] || { echo "Pass full repo name" >&2 ; exit 1 ; }

REPO=$1
docker build --no-cache -f $(dirname $0)/Dockerfile --build-arg BUILD_TIME="$(date)" -t $REPO .
$(aws --region eu-central-1 ecr get-login --no-include-email)
docker push $REPO
docker rmi $REPO
