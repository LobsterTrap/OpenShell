# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

%global crate openshell

# Cargo/Rust builds with vendored deps do not produce debugsource listings
# in the format redhat-rpm-config expects (especially on EPEL).
%global debug_package %{nil}

Name:           openshell
Version:        0.0.20
Release:        1%{?dist}
Summary:        Safe, sandboxed runtimes for autonomous AI agents

License:        Apache-2.0
URL:            https://github.com/LobsterTrap/OpenShell
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  x86_64 aarch64

# Rust build dependencies
# NOTE: MSRV is 1.88 (Rust edition 2024). As of mid-2025, this requires
# Fedora Rawhide or newer. Stable Fedora and EPEL-10 may ship older Rust;
# adjust targets in .packit.yaml accordingly or provide a supplementary
# Rust toolchain via additional_repos in the COPR build config.
BuildRequires:  rust >= 1.88
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  cmake
BuildRequires:  pkg-config

# Python sub-package build dependencies
BuildRequires:  python3-devel

%description
OpenShell provides safe, sandboxed runtimes for autonomous AI agents.
It offers a CLI for managing gateways, sandboxes, and providers with
policy-enforced egress routing, credential proxying, and privacy-aware
LLM inference routing.

# --- Python SDK sub-package ---
%package -n python3-%{name}
Summary:        OpenShell Python SDK for agent execution and management
Requires:       python3-cloudpickle >= 3.0
Requires:       python3-grpcio >= 1.60
Requires:       python3-protobuf >= 4.25
Recommends:     %{name}

%description -n python3-%{name}
Python SDK for OpenShell providing programmatic access to sandbox
management, agent execution, and inference routing via gRPC.

%prep
%autosetup -n %{name}-%{version}

# Extract vendored Cargo dependencies
tar xf %{SOURCE1}

# Configure Cargo to use vendored dependencies for offline build
mkdir -p .cargo
cat > .cargo/config.toml << 'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

# Patch workspace version from placeholder to actual version
sed -i 's/^version = "0.0.0"/version = "%{version}"/' Cargo.toml
grep -q 'version = "%{version}"' Cargo.toml || (echo "ERROR: Cargo.toml version patch failed" && exit 1)

%build
# Build only the CLI binary
export CARGO_BUILD_JOBS=%{_smp_build_ncpus}
cargo build --release --bin openshell

%install
# Install CLI binary
install -Dpm 0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}

# Install Python SDK modules (test files are intentionally excluded)
install -d %{buildroot}%{python3_sitelib}/%{name}
install -d %{buildroot}%{python3_sitelib}/%{name}/_proto

install -pm 0644 python/%{name}/__init__.py %{buildroot}%{python3_sitelib}/%{name}/
install -pm 0644 python/%{name}/sandbox.py %{buildroot}%{python3_sitelib}/%{name}/
install -pm 0644 python/%{name}/_proto/__init__.py %{buildroot}%{python3_sitelib}/%{name}/_proto/
install -pm 0644 python/%{name}/_proto/*.py %{buildroot}%{python3_sitelib}/%{name}/_proto/

# Create dist-info so importlib.metadata can resolve the package version
install -d %{buildroot}%{python3_sitelib}/%{name}-%{version}.dist-info
cat > %{buildroot}%{python3_sitelib}/%{name}-%{version}.dist-info/METADATA << EOF
Metadata-Version: 2.1
Name: %{name}
Version: %{version}
Summary: OpenShell Python SDK for agent execution and management
License: Apache-2.0
Requires-Python: >=3.12
Requires-Dist: cloudpickle>=3.0
Requires-Dist: grpcio>=1.60
Requires-Dist: protobuf>=4.25
EOF

# INSTALLER marker per PEP 376
echo "rpm" > %{buildroot}%{python3_sitelib}/%{name}-%{version}.dist-info/INSTALLER

# RECORD can be empty for RPM-managed installs
touch %{buildroot}%{python3_sitelib}/%{name}-%{version}.dist-info/RECORD

%check
# Smoke-test the CLI binary
%{buildroot}%{_bindir}/%{name} --version

# Smoke-test the Python SDK version metadata via importlib.metadata.
# We query the dist-info directly rather than importing the package because
# the full import pulls in grpcio and other runtime deps not present in the
# build environment.
PYTHONPATH=%{buildroot}%{python3_sitelib} %{python3} -c "from importlib.metadata import version; v = version('openshell'); print(v); assert v == '%{version}', f'expected %{version}, got {v}'"

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}

%files -n python3-%{name}
%license LICENSE
%{python3_sitelib}/%{name}/
%{python3_sitelib}/%{name}-%{version}.dist-info/

%changelog
%autochangelog
