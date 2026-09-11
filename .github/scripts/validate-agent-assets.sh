#!/usr/bin/env bash
set -euo pipefail

# Keep the repository's agent guidance executable and discoverable without
# requiring PyYAML or another non-Rust dependency on CI runners.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
skills="${ROOT}/.agents/skills"
docs="${ROOT}/docs/agent-knowledge"

test -d "${skills}" || { echo "missing ${skills}" >&2; exit 1; }
test -d "${docs}" || { echo "missing ${docs}" >&2; exit 1; }

for skill_dir in "${skills}"/*; do
  [ -d "${skill_dir}" ] || continue
  file="${skill_dir}/SKILL.md"
  test -f "${file}" || { echo "missing SKILL.md: ${skill_dir}" >&2; exit 1; }
  name="$(basename "${skill_dir}")"
  header="$(sed -n 's/^name: //p' "${file}" | head -1)"
  description="$(sed -n 's/^description: //p' "${file}" | head -1)"
  [ "${header}" = "${name}" ] || {
    echo "skill name mismatch: ${file} (${header} != ${name})" >&2
    exit 1
  }
  [ -n "${description}" ] || { echo "missing description: ${file}" >&2; exit 1; }
  if rg -n 'TODO|FIXME|your path here|/path/to|<replace' "${file}"; then
    echo "unfinished placeholder in ${file}" >&2
    exit 1
  fi
done

for doc in "${docs}"/*.md; do
  test -s "${doc}" || { echo "empty knowledge document: ${doc}" >&2; exit 1; }
done

echo "agent skills and knowledge documents are valid"
