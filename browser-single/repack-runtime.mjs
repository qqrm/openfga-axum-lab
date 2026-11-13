// Replace the Linux kernel only; preserve the emulator and application filesystem.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, createWriteStream, promises as fs} from 'node:fs';
import {once} from 'node:events';
import path from 'node:path';
const [runtime, kernel] = process.argv.slice(2);
assert(runtime && kernel, 'usage: node repack-runtime.mjs RUNTIME_DIR KERNEL_DIR');
const loaderPath = path.join(runtime, 'load.js');
const dataPath = path.join(runtime, 'qemu-system-x86_64.data');
const original = await fs.readFile(loaderPath, 'utf8');
const pattern = /loadPackage\((\{"files":.*\})\);/g;
const matches = [...original.matchAll(pattern)];
assert.equal(matches.length, 1, 'Expected one known metadata record');
const metadata = JSON.parse(matches[0][1]);
assert.equal(metadata.remote_package_size, (await fs.stat(dataPath)).size);
assert.equal(metadata.files.filter(f => f.filename === '/pack/bzImage').length, 1);
assert.equal(metadata.files.filter(f => f.filename === '/pack/vm.state').length, 1);
let previous = 0;
for (const file of metadata.files) {
  assert.equal(file.start, previous, 'Input segments must be contiguous');
  assert(Number.isSafeInteger(file.end) && file.end > file.start);
  previous = file.end;
}
assert.equal(previous, metadata.remote_package_size);
const kernelPath = path.join(kernel, 'bzImage');
const kernelSize = (await fs.stat(kernelPath)).size;
assert(kernelSize > 1024 * 1024 && kernelSize < 32 * 1024 * 1024, 'Unexpected kernel size');
const config = await fs.readFile(path.join(kernel, 'linux.config'), 'utf8');
for (const option of ['CONFIG_SYSVIPC=y', 'CONFIG_IPC_NS=y', 'CONFIG_ACPI=y', 'CONFIG_X86_PM_TIMER=y', 'CONFIG_HPET_TIMER=y']) assert(config.split('\n').includes(option), `Missing ${option}`);
const outputPath = `${dataPath}.new`;
const output = createWriteStream(outputPath, {flags:'wx'});
const hash = createHash('sha256');
let position = 0;
const files = [];
try {
  for (const file of metadata.files) {
    // Cold boot never restores the converter's obsolete 128 MiB VM snapshot.
    if (file.filename === '/pack/vm.state') continue;
    const start = position;
    const input = file.filename === '/pack/bzImage'
      ? createReadStream(kernelPath)
      : createReadStream(dataPath, {start:file.start, end:file.end - 1});
    for await (const chunk of input) {
      hash.update(chunk);
      position += chunk.length;
      if (!output.write(chunk)) await once(output, 'drain');
    }
    const expected = file.filename === '/pack/bzImage' ? kernelSize : file.end - file.start;
    assert.equal(position - start, expected, 'Truncated input segment');
    files.push({...file, start, end:position});
  }
  output.end();
  await once(output, 'close');
  const updated = {...metadata, files, remote_package_size:position};
  const loader = original.replace(matches[0][0], `loadPackage(${JSON.stringify(updated)});`);
  await fs.rename(outputPath, dataPath);
  await fs.writeFile(loaderPath, loader);
  await fs.cp(kernel, path.join(runtime, 'kernel-build'), {recursive:true});
  await fs.writeFile(path.join(runtime, 'derived-runtime.json'), JSON.stringify({
    source:'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58',
    linux_source:'830b3c68c1fb1e9176028d02ef86f3cf76aa2476',
    changes:['Linux kernel configuration: SYSVIPC, SYSVIPC_SYSCTL, IPC_NS, ACPI and X86_PM_TIMER', 'remove unused 128 MiB migration snapshot'],
    rootfs_changed:false,
    data:{bytes:position,sha256:hash.digest('hex')},
    loader_sha256:createHash('sha256').update(loader).digest('hex'),
    kernel_sha256:createHash('sha256').update(await fs.readFile(kernelPath)).digest('hex')
  }, null, 2) + '\n');
  console.log(`Repacked ${position} bytes; application filesystem is unchanged`);
} catch(error) {
  output.destroy();
  await fs.rm(outputPath, {force:true});
  throw error;
}
