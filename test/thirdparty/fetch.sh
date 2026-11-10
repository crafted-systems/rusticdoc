#!/usr/bin/env bash
# Downloads the third-party templates this check ports, at pinned commits, and
# verifies their checksums. They are not kept in this repository.
#   test/thirdparty/fetch.sh [dir]   (default test/thirdparty/upstream)
set -euo pipefail
cd "$(dirname "$0")"
dir=${1:-upstream}
xt=https://raw.githubusercontent.com/optilude/xlsx-template/7a95c8f41721b5d2f6c743a12e3f266d37a683da/test/templates
dt=https://raw.githubusercontent.com/open-xml-templating/docxtemplater/e1d61c64cb4a37420e53e57a673f5f36a2e34400/examples
mkdir -p "$dir/xlsx-template" "$dir/docxtemplater"
while read -r sum path; do
  case $path in
    xlsx-template/*) url=$xt/${path#*/} ;;
    docxtemplater/*) url=$dt/${path#*/} ;;
  esac
  curl -sfL "$url" -o "$dir/$path"
  echo "$sum  $dir/$path" | shasum -a 256 --check --quiet
  echo "$path"
done <<'SUMS'
2c429f010e359443a4fae4d883700ac8fea9d5f1f2e403bf42580c0bf59e7876 xlsx-template/multple-sheets-arrays.xlsx
937f3dfcd9df21a3772983e36b3cb9e5857cbe8211cde089f1affa274f3581ce xlsx-template/t1.xlsx
0f5f47348c909f3e1cd1da6af5a402ea47d5358de19891e2ea809663631de54d xlsx-template/test-array.xlsx
f232adfb1c8eeaccef2ba96ce0deb5d1195b19eedfe93e9722daab2e4f2d7d7e xlsx-template/test-nested-arrays.xlsx
926ac86fc4bcbba2a4821e5df7d41ebfbe59b5c33770afb241ff72ef2a910368 docxtemplater/loop-example.pptx
e4c97e51a9952d977ed1939cbb261360f57d79a2e93804883dda9473f34c63cc docxtemplater/expected-loop-example.pptx
fe6f74257d429067e5185a0c2c0eabd709519b6f28aca8a095c76d69bb2bdaf7 docxtemplater/loop-table.pptx
0a8403df96e7887270e2f5f687a9db2b64e27c6db9abf41ecbc9d93560799d3f docxtemplater/expected-loop-table.pptx
10d1bb4a3c82444b75d55552d948c70be4cf4eb1c34846449997ff6363b58121 docxtemplater/paragraph-loop.pptx
10f4f1111b7afe518076436f46e6254c326cf4812aeabba84eef9bff7aafa06a docxtemplater/expected-paragraph-loop.pptx
SUMS
