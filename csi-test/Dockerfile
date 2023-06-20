FROM golang:1.20.5-bullseye AS builder
WORKDIR /build

RUN apt-get update && apt-get install make unzip -y
COPY ./csi-test-5.0.0.zip .
RUN unzip ./csi-test-5.0.0.zip

WORKDIR /build/csi-test-5.0.0/cmd/csi-sanity

RUN make

FROM debian:bullseye
WORKDIR /runtime

COPY ./parameters.yaml .
COPY ./test.sh .
RUN chmod +x test.sh
COPY --from=builder /build/csi-test-5.0.0/cmd/csi-sanity/csi-sanity /runtime/csi-sanity

ENTRYPOINT ["/bin/sleep", "99999999"]