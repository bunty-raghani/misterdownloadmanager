# ClusterFuzzLite's build of MDM's fuzz targets (fuzz/, cargo fuzz): OSS-Fuzz's Rust image, pinned
# by digest, with the nightly the targets are built with (the workspace needs Rust 1.98 or later,
# newer than the image's own).
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:3428d0a90b1e7efbe2ba947bd1d0c9a70594986b4b6553804547f1fb24cfc92f
RUN rustup toolchain install nightly-2026-11-12 --profile minimal
ENV RUSTUP_TOOLCHAIN=nightly-2026-11-12
# aws-lc's build (the TLS and signature crypto).
RUN apt-get update && apt-get install -y --no-install-recommends cmake
COPY . $SRC/misterdownloadmanager
WORKDIR $SRC/misterdownloadmanager
COPY .clusterfuzzlite/build.sh $SRC/
