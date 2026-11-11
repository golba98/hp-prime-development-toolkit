// Assemble local npm packages. This script never downloads or publishes.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { collectLicenses } from './licenses.mjs';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
async function readableTree(directory){
  await fs.chmod(directory,0o755);
  for(const entry of await fs.readdir(directory,{withFileTypes:true})){
    const file=path.join(directory,entry.name);
    if(entry.isDirectory())await readableTree(file);else if(entry.isFile())await fs.chmod(file,0o644);
  }
}
const args=process.argv.slice(2);
const option=(key,fallback)=>{const i=args.indexOf(key);return i<0?fallback:args[i+1];};
const target=option('--target');const binary=option('--binary');const output=path.resolve(option('--output',path.join(root,'dist')));
const targets={
  'x86_64-unknown-linux-musl':['linux','x64'], 'aarch64-unknown-linux-musl':['linux','arm64'],
  'x86_64-unknown-linux-gnu':['linux','x64'], 'aarch64-unknown-linux-gnu':['linux','arm64'],
  'x86_64-pc-windows-msvc':['win32','x64'], 'aarch64-apple-darwin':['darwin','arm64'], 'x86_64-apple-darwin':['darwin','x64']
};
if(!targets[target]||!binary)throw new Error('Usage: node scripts/package.mjs --target RUST_TARGET --binary PATH [--output DIR]');
const [os,cpu]=targets[target];
const metadata=JSON.parse(await fs.readFile(path.join(root,'npm/package.json'),'utf8'));
const cargoVersion=(await fs.readFile(path.join(root,'Cargo.toml'),'utf8')).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if(metadata.version!==cargoVersion||Object.values(metadata.optionalDependencies).some(v=>v!==cargoVersion))throw new Error('Cargo, npm and platform dependency versions must match exactly.');
const licenseDirectory=path.join(output,'licenses');
await collectLicenses(root,licenseDirectory);
const rootPackage=path.join(output,'npm',metadata.name),name=`hp-prime-devkit-${os}-${cpu}`,platformPackage=path.join(output,'npm',name);
await fs.mkdir(rootPackage,{recursive:true});await fs.cp(path.join(root,'npm'),rootPackage,{recursive:true});
for(const file of ['README.md','LICENSE','THIRD_PARTY_NOTICES.md'])await fs.copyFile(path.join(root,file),path.join(rootPackage,file));
await fs.cp(path.join(root,'examples'),path.join(rootPackage,'examples'),{recursive:true});
await fs.cp(path.join(root,'docs'),path.join(rootPackage,'docs'),{recursive:true});
await fs.cp(path.join(root,'tests'),path.join(rootPackage,'tests'),{recursive:true});
await fs.copyFile(path.join(root,'hpdev.toml'),path.join(rootPackage,'hpdev.toml'));
await readableTree(rootPackage);await fs.chmod(path.join(rootPackage,'bin/hpdev.cjs'),0o755);
await fs.mkdir(path.join(platformPackage,'bin'),{recursive:true});
await fs.writeFile(path.join(platformPackage,'package.json'),JSON.stringify({name,version:metadata.version,description:`HP Prime Development Toolkit native executable (${target})`,license:'MIT',os:[os],cpu:[cpu],files:['bin','licenses','LICENSE','THIRD_PARTY_NOTICES.md','SHA256SUMS']},null,2)+'\n');
const executable=os==='win32'?'hpdev.exe':'hpdev';const destination=path.join(platformPackage,'bin',executable);
await fs.copyFile(path.resolve(binary),destination);if(os!=='win32')await fs.chmod(destination,0o755);
const digest=createHash('sha256').update(await fs.readFile(destination)).digest('hex');
await fs.writeFile(path.join(platformPackage,'SHA256SUMS'),`${digest}  bin/${executable}\n`);
for(const file of ['LICENSE','THIRD_PARTY_NOTICES.md'])await fs.copyFile(path.join(root,file),path.join(platformPackage,file));
await fs.cp(licenseDirectory,path.join(platformPackage,'licenses'),{recursive:true});
await readableTree(platformPackage);if(os!=='win32')await fs.chmod(destination,0o755);
const nativeDirectory=path.join(output,`hpdev-${metadata.version}-${target}`);await fs.mkdir(nativeDirectory,{recursive:true});
await fs.copyFile(destination,path.join(nativeDirectory,executable));if(os!=='win32')await fs.chmod(path.join(nativeDirectory,executable),0o755);
for(const file of ['README.md','LICENSE','THIRD_PARTY_NOTICES.md'])await fs.copyFile(path.join(root,file),path.join(nativeDirectory,file));
await fs.cp(licenseDirectory,path.join(nativeDirectory,'licenses'),{recursive:true});await fs.cp(path.join(root,'examples'),path.join(nativeDirectory,'examples'),{recursive:true});
await fs.cp(path.join(root,'docs'),path.join(nativeDirectory,'docs'),{recursive:true});
await fs.cp(path.join(root,'tests'),path.join(nativeDirectory,'tests'),{recursive:true});
await fs.copyFile(path.join(root,'hpdev.toml'),path.join(nativeDirectory,'hpdev.toml'));
await fs.writeFile(path.join(nativeDirectory,'SHA256SUMS'),`${digest}  ${executable}\n`);
await readableTree(nativeDirectory);if(os!=='win32')await fs.chmod(path.join(nativeDirectory,executable),0o755);
const archive=`${nativeDirectory}.tar.gz`;execFileSync('tar',['-czf',archive,'-C',path.dirname(nativeDirectory),path.basename(nativeDirectory)]);
const archiveDigest=createHash('sha256').update(await fs.readFile(archive)).digest('hex');await fs.writeFile(`${archive}.sha256`,`${archiveDigest}  ${path.basename(archive)}\n`);
console.log(JSON.stringify({version:metadata.version,target,rootPackage,platformPackage,nativeArchive:archive,sha256:digest}));
