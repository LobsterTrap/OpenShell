# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Example: a minimal Python REST API that runs inside a OpenShell sandbox.
# The image exposes port 8080 with a /hello endpoint.  Use port forwarding
# (--forward 8080 on the CLI) to reach it from your local machine.

FROM quay.io/hummingbird/python:3.13-builder

# System tools useful for sandbox networking and debugging.
# iproute2: required for network namespace management (ip netns, veth pairs)
# iptables: optional, enables bypass detection (LOG + REJECT for direct connections)
RUN dnf install -y curl iproute iptables && dnf clean all

# Create the sandbox user (uid/gid 1000) for non-root execution.
RUN groupadd -g 1000 sandbox && \
    useradd -m -u 1000 -g sandbox sandbox

WORKDIR /sandbox
COPY app.py .

EXPOSE 8080

# NOTE: The sandbox supervisor replaces CMD at runtime.  Pass the start
# command explicitly:  openshell sandbox create ... -- python /sandbox/app.py
CMD ["python", "app.py"]
