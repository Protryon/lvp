#!/bin/bash
set -ex
here=$(realpath $(dirname "$0"))
cd "$here"

if [ -z ${1+x} ] ; then
    echo "missing tag"
    exit 1
fi

export TAG=$1

docker build -t protryon/lvp:$TAG -f ./Dockerfile .
docker push protryon/lvp:$TAG
docker image rm protryon/lvp:$TAG

echo "Uploaded image protryon/lvp:$TAG"
