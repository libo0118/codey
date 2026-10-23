import { execFile } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, open, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { readSourceVersion } from "./prepare-release-version.mjs";
import { releaseIdentity } from "./release-policy.mjs";

const execute = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const documents = [
  ["README.md", "README.md"],
  ["LICENSE", "LICENSE"],
  ["THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"],
  ["licenses/FastCtx/LICENSE-APACHE", "licenses/FastCtx/LICENSE-APACHE"],
  ["licenses/FastCtx/NOTICE", "licenses/FastCtx/NOTICE"],
  ["vendor/ComputerUse/LICENSE", "licenses/ComputerUse/LICENSE"],
];

export function linuxPackageVersions(version) {
  const { prerelease } = releaseIdentity(version);
  const [core, suffix] = version.split(/-(.*)/s);
  return { deb: prerelease ? `${core}~${suffix}` : version, rpmVersion: core, rpmRelease: suffix ? `${prerelease ? "0" : "1"}.${suffix.replaceAll("-", ".")}` : "1" };
}

export async function stageLinuxInstallTree(bundle, destination) {
  for (const binary of ["codey", "codey-fastctx"]) {
    const target = join(destination, "usr/lib/codey", binary);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(join(bundle, binary), target);
    await chmod(target, 0o755);
  }
  for (const [, document] of documents) {
    const target = join(destination, "usr/share/doc/codey", document);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(join(bundle, document), target);
  }
  const launcher = join(destination, "usr/bin/codey");
  await mkdir(dirname(launcher), { recursive: true });
  await writeFile(launcher, '#!/bin/sh\nexec /usr/lib/codey/codey "$@"\n', { mode: 0o755 });
  await mkdir(join(destination, "usr/share/applications"), { recursive: true });
  await writeFile(join(destination, "usr/share/applications/codey.desktop"), "[Desktop Entry]\nType=Application\nName=Codey\nComment=Codey desktop launcher\nExec=/usr/bin/codey\nTerminal=false\nCategories=Development;\n");
}

async function filesIn(directory, prefix = "") {
  const files = [];
  for (const entry of await readdir(join(directory, prefix), { withFileTypes: true })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...await filesIn(directory, path));
    else if (entry.isFile()) files.push(path);
    else throw new Error(`Unexpected package entry: ${path}`);
  }
  return files.sort();
}

async function verifyExtractedTree(expected, extracted) {
  const expectedFiles = await filesIn(expected);
  const actualFiles = await filesIn(extracted);
  if (JSON.stringify(expectedFiles) !== JSON.stringify(actualFiles)) throw new Error("Linux package contains unexpected or missing files");
  for (const path of expectedFiles) {
    if (!(await readFile(join(expected, path))).equals(await readFile(join(extracted, path)))) throw new Error(`Linux package content mismatch: ${path}`);
    if (((await stat(join(expected, path))).mode & 0o777) !== ((await stat(join(extracted, path))).mode & 0o777)) throw new Error(`Linux package permissions mismatch: ${path}`);
  }
}

async function packageInstallers(bundle, stage, version, output, name) {
  if (process.platform !== "linux") throw new Error("Debian and RPM packaging requires Linux packaging tools");
  const payload = join(stage, "payload");
  await stageLinuxInstallTree(bundle, payload);
  const maintainer = "Codey maintainers <noreply@github.com>";
  const description = "Codey launcher for the official Codex desktop application";
  const versions = linuxPackageVersions(version);
  await mkdir(join(stage, "debian"));
  await writeFile(join(stage, "debian/control"), `Source: codey\nSection: devel\nPriority: optional\nMaintainer: ${maintainer}\n\nPackage: codey\nArchitecture: amd64\nDescription: ${description}\n`);
  const { stdout: shlibs } = await execute("dpkg-shlibdeps", ["-O", ...["codey", "codey-fastctx"].map(binary => `-e${join(payload, "usr/lib/codey", binary)}`)], { cwd: stage });
  const depends = shlibs.match(/^shlibs:Depends=(.*)$/m)?.[1];
  if (!depends) throw new Error("Unable to determine Debian shared-library dependencies");
  const debRoot = join(stage, "deb");
  await stageLinuxInstallTree(bundle, debRoot);
  await mkdir(join(debRoot, "DEBIAN"));
  await writeFile(join(debRoot, "DEBIAN/control"), `Package: codey\nVersion: ${versions.deb}\nArchitecture: amd64\nMaintainer: ${maintainer}\nSection: devel\nPriority: optional\nDepends: ${depends}\nHomepage: https://github.com/libo0118/codey\nDescription: ${description}\n`);
  const deb = join(output, `${name}.deb`);
  await execute("dpkg-deb", ["--root-owner-group", "-Zxz", "--build", debRoot, deb]);
  const { stdout: debIdentity } = await execute("dpkg-deb", ["--field", deb, "Package", "Version", "Architecture", "Depends"]);
  if (!debIdentity.includes(`Version: ${versions.deb}\n`) || !debIdentity.includes("Architecture: amd64\n") || !debIdentity.includes(`Depends: ${depends}\n`)) throw new Error("Debian package metadata does not match the build");
  await execute("dpkg-deb", ["--contents", deb]);
  const debExtracted = join(stage, "deb-extracted");
  await execute("dpkg-deb", ["--extract", deb, debExtracted]);
  await verifyExtractedTree(payload, debExtracted);

  const rpmRoot = join(stage, "rpm");
  for (const directory of ["BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"]) await mkdir(join(rpmRoot, directory), { recursive: true });
  const spec = join(rpmRoot, "SPECS/codey.spec");
  // Preserve the same ELF bytes in all formats; the Rust release build owns stripping.
  await writeFile(spec, `%global debug_package %{nil}\n%global _build_id_links none\n%global _binary_payload w9.gzdio\n%global __strip /bin/true\nName: codey\nVersion: ${versions.rpmVersion}\nRelease: ${versions.rpmRelease}\nSummary: ${description}\nLicense: AGPL-3.0-only AND Apache-2.0 AND MIT\nURL: https://github.com/libo0118/codey\nBuildArch: x86_64\nRequires: /bin/sh\n\n%description\n${description}.\n\n%install\nmkdir -p "%{buildroot}"\ncp -a "${payload}/." "%{buildroot}/"\n\n%files\n%defattr(-,root,root,-)\n/usr/bin/codey\n/usr/lib/codey\n/usr/share/applications/codey.desktop\n%doc /usr/share/doc/codey\n`);
  const rpm = join(output, `${name}.rpm`);
  await execute("rpmbuild", ["-bb", "--target", "x86_64", "--define", `_topdir ${rpmRoot}`, "--define", `_rpmdir ${output}`, "--define", `_rpmfilename ${name}.rpm`, spec], { maxBuffer: 8 * 1024 * 1024 });
  const { stdout: rpmIdentity } = await execute("rpm", ["-qp", "--queryformat", "%{NAME} %{VERSION} %{RELEASE} %{ARCH}\n", rpm]);
  if (rpmIdentity.trim() !== `codey ${versions.rpmVersion} ${versions.rpmRelease} x86_64`) throw new Error("RPM package metadata does not match the build");
  await execute("rpm", ["-qpl", rpm]);
  const { stdout: requires } = await execute("rpm", ["-qp", "--requires", rpm]);
  if (!requires.includes("libc.so.6")) throw new Error("RPM package is missing automatic ELF shared-library dependencies");
  const rpmExtracted = join(stage, "rpm-extracted");
  await mkdir(rpmExtracted);
  await execute("bash", ["-o", "pipefail", "-c", 'rpm2cpio "$1" | cpio -id --quiet', "codey-unpack", rpm], { cwd: rpmExtracted });
  await verifyExtractedTree(payload, rpmExtracted);
}

async function validateExecutable(path) {
  const file = await open(path, "r");
  try {
    const header = Buffer.alloc(20);
    const { bytesRead } = await file.read(header, 0, header.length, 0);
    if (!(await file.stat()).isFile() || bytesRead !== 20 || !header.subarray(0, 7).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1])) || header.readUInt16LE(18) !== 62) {
      throw new Error(`Expected a Linux x64 ELF executable: ${path}`);
    }
  } finally {
    await file.close();
  }
}

export async function packageLinux(projectRoot = root, { installers = false } = {}) {
  const version = await readSourceVersion(projectRoot);
  const name = `Codey-${version}-linux-x64`;
  const output = join(projectRoot, "dist", "linux");
  const archive = join(output, `${name}.tar.gz`);
  await mkdir(output, { recursive: true });
  const outputs = [archive, ...(installers ? [join(output, `${name}.deb`), join(output, `${name}.rpm`)] : [])];
  for (const path of outputs) if (await stat(path).then(() => true, error => {
    if (error.code !== "ENOENT") throw error;
    return false;
  })) throw new Error(`Package already exists: ${path}`);
  const stage = await mkdtemp(join(output, ".package-"));
  try {
    const bundle = join(stage, name);
    await mkdir(bundle);
    for (const binary of ["codey", "codey-fastctx"]) {
      const source = join(projectRoot, "target", "release", binary);
      await validateExecutable(source);
      await copyFile(source, join(bundle, binary));
      await chmod(join(bundle, binary), 0o755);
    }
    for (const [source, destination] of documents) {
      const destinationPath = join(bundle, destination);
      await mkdir(dirname(destinationPath), { recursive: true });
      await copyFile(join(projectRoot, source), destinationPath);
    }
    await execute("tar", ["-czf", archive, "-C", stage, name]);
    if (installers) await packageInstallers(bundle, stage, version, output, name);
  } catch (error) {
    for (const path of outputs) await rm(path, { force: true });
    throw error;
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
  return archive;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const version = await readSourceVersion(root);
  if (process.env.GITHUB_REF_TYPE === "tag" && process.env.GITHUB_REF_NAME !== `v${version}`) throw new Error("Release tag must match package.json version");
  console.log(await packageLinux(root, { installers: true }));
}
