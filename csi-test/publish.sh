#!/bin/bash
set -ex
here=$(realpath $(dirname "$0"))
cd "$here"

if [ -z ${1+x} ] ; then
    echo "missing tag"
    exit 1
fi

export TAG=$1

docker build -t protryon/csi-test:$TAG -f ./Dockerfile .
docker push protryon/csi-test:$TAG
docker image rm protryon/csi-test:$TAG

echo "Uploaded image protryon/csi-test:$TAG"
