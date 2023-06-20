fn main() {
    tonic_build::compile_protos("proto/csi.proto").unwrap();
}
