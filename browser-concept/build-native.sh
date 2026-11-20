#!/usr/bin/env bash
# Build only the new concept executable; retained VM/runtime inputs are separate.
set -euo pipefail
umask 022
repo_root=$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
cd "$repo_root"
: "${GITHUB_SHA:?the exact producer commit is required}"
: "${GITHUB_RUN_ID:?the producer run is required}"
: "${GITHUB_RUN_ATTEMPT:?the producer attempt is required}"
[[ "$GITHUB_RUN_ATTEMPT" == 1 ]]
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
git diff --exit-code HEAD --
native_output="$repo_root/artifacts/browser-concept-native"
test ! -e "$native_output"
mkdir -p "$native_output"

python3 - "$native_output" <<'PY'
import hashlib,json,pathlib,stat,subprocess,sys
output=pathlib.Path(sys.argv[1])
paths=subprocess.check_output(['git','ls-files','-z']).decode().split('\0')
directories=('src/','tools/','assets/','db/','migrations/','fga/','benches/')
selected=[p for p in paths if p and (p in ['.dockerignore','Cargo.toml','Cargo.lock','rust-toolchain.toml','browser-concept/Dockerfile','browser-concept/build-native.sh']
    or p.startswith(directories) or (p.startswith('browser-concept/') and '/' not in p[len('browser-concept/'):] and p.endswith('.json')))]
assert 'src/bin/browser-concept.rs' in selected and 'browser-concept/model.json' in selected
files={}
for name in sorted(selected):
    path=pathlib.Path(name);info=path.lstat();assert stat.S_ISREG(info.st_mode),name
    raw=path.read_bytes();files[name]={'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}
proof={'sourceCommit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
       'sourceTree':subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip(),'files':files}
(output/'source-manifest.json').write_text(json.dumps(proof,indent=2)+'\n')
PY

# Resolve the reviewed compiler tag once, then build from its exact pulled digest.
builder_tag=rust:1.99.1-slim-bookworm
docker pull --platform linux/amd64 "$builder_tag"
docker image inspect "$builder_tag" > "$native_output/builder-image.json"
builder_digest=$(docker image inspect "$builder_tag" --format '{{index .RepoDigests 0}}')
[[ "$builder_digest" =~ ^rust@sha256:[0-9a-f]{64}$ ]]
docker buildx build --platform linux/amd64 --progress plain \
    --file browser-concept/Dockerfile --target output \
    --build-arg "RUST_IMAGE=$builder_digest" \
    --output "type=local,dest=$native_output" . \
    2>&1 | tee "$native_output/build.log"

python3 - "$native_output" "$builder_tag" "$builder_digest" <<'PY'
import hashlib,json,os,pathlib,re,struct,subprocess,sys
output=pathlib.Path(sys.argv[1]);tag,digest=sys.argv[2:]
def record(path):
    raw=path.read_bytes();return {'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}
source=json.loads((output/'source-manifest.json').read_text())
assert source['sourceCommit']==os.environ['GITHUB_SHA']
for name,expected in source['files'].items():assert record(pathlib.Path(name))==expected,name
images=json.loads((output/'builder-image.json').read_text());assert len(images)==1
image=images[0];assert image['Os']=='linux' and image['Architecture']=='amd64' and digest in image['RepoDigests']
binary=(output/'concept').read_bytes()
assert binary[:6]==b'\x7fELF\x02\x01' and struct.unpack_from('<H',binary,18)[0]==62,'Expected Linux x86-64 ELF'
versions=[tuple(map(int,x.split('.'))) for x in re.findall(r'GLIBC_(\d+(?:\.\d+)+)',(output/'elf.txt').read_text())]
assert versions and max(versions)<=(2,36),'Binary requires a newer libc than the retained Bookworm guest'
tests=(output/'unit-tests.log').read_text()
match=re.search(r'test result: ok\. ([1-9][0-9]*) passed; 0 failed;',tests)
assert match,'Focused concept behavioral tests did not report a nonempty successful suite'
compiler=(output/'rustc-version.txt').read_text();assert re.search(r'^release: 1\.99\.1$',compiler,re.M)
checks={name:{'passed':True,'log':record(output/log)} for name,log in
        [('rustfmt','rustfmt.log'),('clippy','clippy.log'),('unitTests','unit-tests.log')]}
checks['unitTests']['passedTests']=int(match[1])
proof={'schemaVersion':1,'kind':'browser_concept_native','sourceCommit':source['sourceCommit'],'sourceTree':source['sourceTree'],
       'target':'x86_64-unknown-linux-gnu','binary':record(output/'concept'),'model':record(pathlib.Path('browser-concept/model.json')),
       'cargoToml':record(pathlib.Path('Cargo.toml')),'cargoLock':record(pathlib.Path('Cargo.lock')),
       'sourceManifest':record(output/'source-manifest.json'),'sourceInputCount':len(source['files']),
       'builder':{'tag':tag,'digest':digest,'imageId':image['Id'],'inspection':record(output/'builder-image.json')},
       'compiler':compiler,'cargo':(output/'cargo-version.txt').read_text().strip(),
       'buildCommand':'cargo build --locked --release --bin browser-concept','checks':checks,
       'maximumRequiredGlibc':'.'.join(map(str,max(versions))),
       'producer':{'runId':int(os.environ['GITHUB_RUN_ID']),'runAttempt':int(os.environ['GITHUB_RUN_ATTEMPT']),
                   'workflowCommit':os.environ['GITHUB_SHA'],'jobName':'Build browser concept binary'},
       'nativeIntegrationVerified':False,'browserAcceptanceVerified':False,'rootfsRebuilt':False,'emulatorRebuilt':False}
(output/'build.json').write_text(json.dumps(proof,indent=2)+'\n')
subprocess.run(['git','diff','--exit-code','HEAD','--'],check=True)
print(json.dumps({'kind':proof['kind'],'sourceCommit':proof['sourceCommit'],'binary':proof['binary'],
                  'passedUnitTests':checks['unitTests']['passedTests'],'nativeIntegrationVerified':False}))
PY
