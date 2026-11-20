# Restore the immutable artifact from its successful build job; never relabel it as rebuilt.
import hashlib,json,os,pathlib,stat,urllib.error,urllib.request,zipfile
from urllib.parse import urlsplit
repo='qqrm/openfga-axum-lab'
root=pathlib.Path('prepared');destination=pathlib.Path('candidate-emulator')
aid=int(os.environ['CANDIDATE_ARTIFACT_ID']);expected_sha=os.environ['CANDIDATE_ARTIFACT_DIGEST'].removeprefix('sha256:')
run=43969702163;attempt=1;head='a12b8799fed16e6efc93ab55498e8f165fb1a139'
assert os.environ['GITHUB_RUN_ATTEMPT']=='1'
assert aid==14276419843 and expected_sha=='6fcd65f16a8a05207b7a15460ae2097ecafa00ee031fcc4c60bda3edf5e1ea30'
assert aid>0 and run>0 and attempt==1 and len(expected_sha)==64
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):return None
opener=urllib.request.build_opener(NoRedirect())
def request(suffix):
    return urllib.request.Request('https://api.github.com/repos/'+repo+suffix,headers={
        'Authorization':'Bearer '+os.environ['ARTIFACT_READ_TOKEN'],'Accept':'application/vnd.github+json',
        'X-GitHub-Api-Version':'2022-11-28','User-Agent':'pr36-emulator-candidate'})
def api(suffix):
    with opener.open(request(suffix),timeout=20) as response:raw=response.read(1024*1024+1)
    assert len(raw)<=1024*1024
    return json.loads(raw)
producer=api('/actions/runs/'+str(run))
assert producer['id']==run and producer['run_attempt']==attempt and producer['head_sha']==head
assert producer['head_branch']=='diag/pr36-idle-worker-gc-20261117'
assert producer['path']=='.github/workflows/pr36-idle-gc-gate.yml'
assert producer['status']=='completed' and producer['conclusion']=='failure' and producer['event']=='push'
a=api('/actions/artifacts/'+str(aid))
assert a['id']==aid and a['name']=='pr36-idle-gc-emulator' and not a['expired']
assert a['workflow_run']['id']==run and a['workflow_run']['head_sha']==head
assert a['size_in_bytes']==16286946 and a['digest']=='sha256:'+expected_sha
jobs=api('/actions/runs/'+str(run)+'/attempts/'+str(attempt)+'/jobs?per_page=100')
assert jobs['total_count']<=100
matches=[j for j in jobs['jobs'] if j['name']=='Build QEMU idle-worker GC candidate']
assert len(matches)==1
job=matches[0]
assert job['id']==133658307719 and job['run_id']==run
assert job['status']=='completed' and job['conclusion']=='success' and job['head_sha']==head
try:
    opener.open(request('/actions/artifacts/'+str(aid)+'/zip'),timeout=20)
    raise RuntimeError('Expected the signed artifact redirect')
except urllib.error.HTTPError as error:
    assert error.code==302
    location=error.headers['Location'];error.close()
u=urlsplit(location);host=u.hostname or ''
assert u.scheme=='https' and u.port in (None,443) and u.username is None and u.password is None
assert host.endswith('.blob.core.windows.net') or host.endswith('.actions.githubusercontent.com')
archive=root/'candidate-emulator.zip';total=0;digest=hashlib.sha256()
with opener.open(urllib.request.Request(location),timeout=30) as response,archive.open('xb') as out:
    while block:=response.read(1024*1024):
        total+=len(block);assert total<=a['size_in_bytes'];digest.update(block);out.write(block)
assert total==a['size_in_bytes'] and digest.hexdigest()==expected_sha
destination.mkdir(exist_ok=False);members={};total=0
with zipfile.ZipFile(archive) as package:
    names=package.namelist();assert len(names)==len(set(names)) and len(names)<=512
    for item in package.infolist():
        name=pathlib.PurePosixPath(item.filename)
        assert not name.is_absolute() and '..' not in name.parts and '\\' not in item.filename
        assert not stat.S_ISLNK(item.external_attr>>16)
        if item.is_dir():continue
        assert 0<=item.file_size<=128*1024*1024
        total+=item.file_size;assert total<=256*1024*1024
        target=destination.joinpath(*name.parts);target.parent.mkdir(parents=True,exist_ok=True)
        count=0;h=hashlib.sha256()
        with package.open(item) as src,target.open('xb') as out:
            while block:=src.read(1024*1024):
                count+=len(block);assert count<=item.file_size;h.update(block);out.write(block)
        assert count==item.file_size
        members[item.filename]={'bytes':count,'sha256':h.hexdigest()}
record={'artifact':aid,'archive':{'bytes':archive.stat().st_size,'sha256':expected_sha},
        'run':run,'attempt':attempt,'workflowCommit':head,'buildJob':job,'members':members,
        'reuseConsumer':{'run':int(os.environ['GITHUB_RUN_ID']),'attempt':1,'workflowCommit':os.environ['GITHUB_SHA']}}
(root/'candidate-emulator-archive-provenance.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'candidateArtifact':aid,'sha256':expected_sha,'members':len(members),'buildJob':job['id']}))
