fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only needed when the gRPC access boundary is enabled (P5-01).
    #[cfg(feature = "grpc-backend")]
    {
        let protoc = protoc_bin_vendored::protoc_bin_path()?;
        // SAFETY: build script runs single-threaded before codegen; setting
        // PROTOC points prost-build at the vendored protoc binary.
        unsafe { std::env::set_var("PROTOC", protoc) };
        tonic_build::configure()
            .compile_protos(&["proto/ontolith/v1/sparql.proto"], &["proto"])?;
    }
    Ok(())
}
