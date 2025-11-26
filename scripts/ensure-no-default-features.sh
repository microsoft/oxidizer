#!/bin/bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

set -euo pipefail

echo "Checking that all workspace dependencies have default-features = false"

# Extract the [workspace.dependencies] section and check each dependency
in_workspace_deps=false
errors=0

while IFS= read -r line; do
  # Check if we're entering the [workspace.dependencies] section
  if [[ "$line" =~ ^\[workspace\.dependencies\] ]]; then
    in_workspace_deps=true
    continue
  fi

  # Check if we're entering a different section (exit workspace.dependencies)
  if [[ "$line" =~ ^\[.*\] ]] && [[ "$in_workspace_deps" == true ]]; then
    break
  fi

  # Process lines in [workspace.dependencies]
  if [[ "$in_workspace_deps" == true ]]; then
    # Skip empty lines and comments
    if [[ -z "$line" ]] || [[ "$line" =~ ^[[:space:]]*# ]]; then
      continue
    fi

    # Check if this is a dependency line (contains '=')
    if [[ "$line" =~ ^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*= ]]; then
      # Extract the dependency name
      dep_name=$(echo "$line" | sed 's/^\s*\([a-zA-Z0-9_-]*\)\s*=.*/\1/')

      # Check if the line contains 'default-features'
      if [[ ! "$line" =~ default-features[[:space:]]*=[[:space:]]*false ]]; then
        echo "ERROR: Dependency '$dep_name' does not specify 'default-features = false'"
        echo "  Line: $line"
        errors=$((errors + 1))
      fi
    fi
  fi
done < Cargo.toml

if [ $errors -gt 0 ]; then
  echo ""
  echo "Found $errors dependencies without 'default-features = false'"
  echo "All workspace dependencies must specify 'default-features = false'"
  exit 1
else
  echo "✓ All workspace dependencies correctly specify 'default-features = false'"
fi
