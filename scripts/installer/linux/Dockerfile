FROM registry.access.redhat.com/ubi8/ubi:8.10

RUN dnf install -y --setopt=install_weak_deps=False \
      gcc gcc-c++ make pkgconf-pkg-config ca-certificates curl git \
      perl python3 tar gzip xz unzip findutils diffutils which \
    && dnf clean all
