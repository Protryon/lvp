set -ex
here=$(realpath $(dirname "$0"))
cd "$here"


helm package -d ./helm_upload/ ./charts/lvp

helm push ./helm_upload/*.tgz oci://registry-1.docker.io/protryon

rm -rf helm_upload