FROM myoung34/github-runner:ubuntu-jammy

USER root

ARG GITLEAKS_VERSION=8.24.3
ARG OSV_SCANNER_VERSION=2.3.3

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    wget \
    jq \
    ripgrep \
    git \
    unzip \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "${arch}" in \
      amd64) \
        gitleaks_arch="x64"; \
        osv_asset="osv-scanner_linux_amd64"; \
        ;; \
      arm64) \
        gitleaks_arch="arm64"; \
        osv_asset="osv-scanner_linux_arm64"; \
        ;; \
      *) \
        echo "Unsupported architecture: ${arch}" >&2; \
        exit 1; \
        ;; \
    esac; \
    curl -fsSL "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/gitleaks_${GITLEAKS_VERSION}_linux_${gitleaks_arch}.tar.gz" \
      | tar -xz -C /usr/local/bin gitleaks; \
    chmod +x /usr/local/bin/gitleaks; \
    curl -fsSL "https://github.com/google/osv-scanner/releases/download/v${OSV_SCANNER_VERSION}/${osv_asset}" \
      -o /usr/local/bin/osv-scanner; \
    chmod +x /usr/local/bin/osv-scanner
