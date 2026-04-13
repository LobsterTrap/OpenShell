#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Common shell functions shared across task scripts

# Read lines into an array variable (bash 3 & 4 compatible)
# Usage: read_lines_into_array array_name < <(command)
#
# Example:
#   read_lines_into_array my_files < <(ls *.txt)
#   for file in "${my_files[@]}"; do
#     echo "$file"
#   done
read_lines_into_array() {
  local array_name=$1
  if ((BASH_VERSINFO[0] >= 4)); then
    # Bash 4+: use mapfile (faster)
    mapfile -t "$array_name"
  else
    # Bash 3: use while loop (macOS default bash is 3.x)
    local line
    eval "$array_name=()"
    while IFS= read -r line; do
      eval "$array_name+=(\"\$line\")"
    done
  fi
}
