import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { linuxPackageVersions, packageLinux, stageLinuxInstallTree } from "../scripts/package-linux.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const licenseFiles = [
  ["README.md", "README.md"],
  ["LICENSE", "LICENSE"],
  ["THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"],
  ["licenses/FastCtx/LICENSE-APACHE", "licenses/FastCtx/LICENSE-APACHE"],
  ["licenses/FastCtx/NOTICE", "licenses/FastCtx/NOTICE"],
  ["vendor/ComputerUse/LICENSE", "licenses/ComputerUse/LICENSE"],
];

async function fixture(directory) {
  await writeFile(join(directory, "package.json"), '{"version":"1.2.3-libo.1"}');
  await writeFile(join(directory, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3-libo.1"\n');
  await mkdir(join(directory, "target/release"), { recursive: true });
  const binary = Buffer.alloc(64);
  binary.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
  binary.writeUInt16LE(62, 18);
  for (const name of ["codey", "codey-fastctx"]) await writeFile(join(directory, "target/release", name), Buffer.concat([binary, Buffer.from(name)]));
  for (const [source] of licenseFiles) {
    await mkdir(dirname(join(directory, source)), { recursive: true });
    await copyFile(join(root, source), join(directory, source));
  }
  await writeFile(join(directory, "auth.json"), '{"fixture":"must not ship"}');
  await writeFile(join(directory, "target/release/config.toml"), "fixture = true");
}

test("Linux x64 archive contains only runnable binaries and existing license documents", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-linux-package-"));
  try {
    await fixture(directory);
    const archive = await packageLinux(directory);
    assert.equal(archive, join(directory, "dist/linux/Codey-1.2.3-libo.1-linux-x64.tar.gz"));
    const prefix = "Codey-1.2.3-libo.1-linux-x64";
    const members = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" }).trim().split(/\r?\n/);
    assert.deepEqual(members.filter(name => !name.endsWith("/")).sort(), [
      ...["codey", "codey-fastctx"].map(name => `${prefix}/${name}`),
      ...licenseFiles.map(([, destination]) => `${prefix}/${destination}`),
    ].sort());
    const extracted = join(directory, "extracted");
    await mkdir(extracted);
    execFileSync("tar", ["-xzf", archive, "-C", extracted]);
    for (const [source, destination] of licenseFiles) assert.deepEqual(await readFile(join(extracted, prefix, destination)), await readFile(join(root, source)));
    for (const name of ["codey", "codey-fastctx"]) {
      const executable = join(extracted, prefix, name);
      assert.deepEqual(await readFile(executable), await readFile(join(directory, "target/release", name)));
      if (process.platform !== "win32") assert.equal((await stat(executable)).mode & 0o777, 0o755);
    }
    const installer = join(directory, "Codey-1.2.3-libo.1-windows-x64-setup.exe");
    await writeFile(installer, "windows fixture");
    const manifestPath = join(directory, "latest.json");
    execFileSync(process.execPath, [join(root, "scripts/generate-update-manifest.mjs"),
      "--version", "1.2.3-libo.1", "--tag", "v1.2.3-libo.1",
      "--download-base-url", "https://github.com/libo0118/codey/releases/download/v1.2.3-libo.1",
      "--output", manifestPath, installer, archive]);
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    assert.deepEqual(manifest.assets.map(asset => [asset.platform, asset.arch, asset.package_type]), [["windows", "x64", "nsis"], ["linux", "x64", "tar-gz"]]);
    assert.equal(manifest.assets[1].sha256, createHash("sha256").update(await readFile(archive)).digest("hex"));
    assert.equal(manifest.assets[1].size, (await stat(archive)).size);
    assert.deepEqual(await readdir(join(directory, "dist/linux")), [`${prefix}.tar.gz`]);
    await assert.rejects(packageLinux(directory), /already exists/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Linux packaging rejects mismatched versions and a non-x64 ELF before producing an archive", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-linux-package-invalid-"));
  try {
    await fixture(directory);
    await writeFile(join(directory, "Cargo.toml"), '[workspace.package]\nversion = "1.2.4"\n');
    await assert.rejects(packageLinux(directory), /版本必须一致/);
    await writeFile(join(directory, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3-libo.1"\n');
    const armBinary = await readFile(join(directory, "target/release/codey-fastctx"));
    armBinary.writeUInt16LE(183, 18);
    await writeFile(join(directory, "target/release/codey-fastctx"), armBinary);
    await assert.rejects(packageLinux(directory), /Linux x64 ELF/);
    await writeFile(join(directory, "target/release/codey-fastctx"), "not a Linux x64 binary");
    await assert.rejects(packageLinux(directory), /Linux x64 ELF/);
    assert.deepEqual(await readdir(join(directory, "dist/linux")), []);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Linux package versions keep fork releases stable and order beta packages before stable", () => {
  assert.deepEqual(linuxPackageVersions("1.2.3-libo.1"), { deb: "1.2.3-libo.1", rpmVersion: "1.2.3", rpmRelease: "1.libo.1" });
  assert.deepEqual(linuxPackageVersions("1.2.3-beta.1"), { deb: "1.2.3~beta.1", rpmVersion: "1.2.3", rpmRelease: "0.beta.1" });
  assert.deepEqual(linuxPackageVersions("1.2.3"), { deb: "1.2.3", rpmVersion: "1.2.3", rpmRelease: "1" });
});

test("Linux installation layout provides a command and desktop entry without user data or lifecycle scripts", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-linux-layout-"));
  try {
    await fixture(directory);
    const archive = await packageLinux(directory);
    const unpacked = join(directory, "unpacked");
    await mkdir(unpacked);
    execFileSync("tar", ["-xzf", archive, "-C", unpacked]);
    const installed = join(directory, "installed");
    await stageLinuxInstallTree(join(unpacked, "Codey-1.2.3-libo.1-linux-x64"), installed);
    const files = (await readdir(installed, { recursive: true, withFileTypes: true })).filter(entry => entry.isFile()).map(entry => join(entry.parentPath, entry.name).slice(installed.length + 1).replaceAll("\\", "/")).sort();
    assert.deepEqual(files, ["usr/bin/codey", "usr/lib/codey/codey", "usr/lib/codey/codey-fastctx", "usr/share/applications/codey.desktop", ...licenseFiles.map(([, path]) => `usr/share/doc/codey/${path}`)].sort());
    assert.equal(await readFile(join(installed, "usr/bin/codey"), "utf8"), '#!/bin/sh\nexec /usr/lib/codey/codey "$@"\n');
    const desktop = await readFile(join(installed, "usr/share/applications/codey.desktop"), "utf8");
    assert.match(desktop, /^Exec=\/usr\/bin\/codey$/m);
    assert.doesNotMatch(desktop, /no-sandbox|autostart/i);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

const nativeTools = ["cc", "dpkg-deb", "dpkg-shlibdeps", "rpmbuild", "rpm", "rpm2cpio", "cpio", "bash"];
const missingNativeTools = process.platform === "linux" ? nativeTools.filter(command => spawnSync("sh", ["-c", 'command -v "$1"', "codey-tool-check", command], { stdio: "ignore" }).status !== 0) : ["Linux"];
test("Linux native package tools are present when required by CI", () => {
  if (process.env.CODEY_REQUIRE_LINUX_PACKAGING_TOOLS === "1") assert.deepEqual(missingNativeTools, []);
});

test("native Debian and RPM packages retain ELF bytes, metadata, dependencies, and the install layout", { skip: missingNativeTools.length ? `Unavailable packaging tools: ${missingNativeTools.join(", ")}` : false }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-linux-native-"));
  try {
    await fixture(directory);
    const source = join(directory, "main.c");
    await writeFile(source, '#include <stdio.h>\nint main(void) { puts("Codey packaging fixture"); return 0; }\n');
    execFileSync("cc", [source, "-o", join(directory, "target/release/codey")]);
    await copyFile(join(directory, "target/release/codey"), join(directory, "target/release/codey-fastctx"));
    await packageLinux(directory, { installers: true });
    const deb = join(directory, "dist/linux/Codey-1.2.3-libo.1-linux-x64.deb");
    const rpm = join(directory, "dist/linux/Codey-1.2.3-libo.1-linux-x64.rpm");
    assert.match(execFileSync("dpkg-deb", ["--info", deb], { encoding: "utf8" }), /Architecture: amd64/);
    assert.match(execFileSync("dpkg-deb", ["--contents", deb], { encoding: "utf8" }), /usr\/lib\/codey\/codey-fastctx/);
    assert.match(execFileSync("dpkg-deb", ["--field", deb, "Depends"], { encoding: "utf8" }), /libc6 \(>= /);
    assert.match(execFileSync("rpm", ["-qpi", rpm], { encoding: "utf8" }), /Architecture\s*:\s*x86_64/);
    assert.match(execFileSync("rpm", ["-qpl", rpm], { encoding: "utf8" }), /\/usr\/bin\/codey/);
    assert.ok(execFileSync("rpm", ["-qp", "--queryformat", "[%{FILEUSERNAME} %{FILEGROUPNAME}\\n]", rpm], { encoding: "utf8" }).trim().split(/\r?\n/).every(owner => owner === "root root"));
    assert.match(execFileSync("rpm", ["-qp", "--requires", rpm], { encoding: "utf8" }), /libc\.so\.6/);
    execFileSync("dpkg", ["--compare-versions", linuxPackageVersions("1.2.3-beta.1").deb, "lt", "1.2.3"]);
    assert.equal(execFileSync("rpm", ["--eval", '%{lua:print(rpm.vercmp("1.2.3-0.beta.1", "1.2.3-1"))}'], { encoding: "utf8" }).trim(), "-1");
    assert.deepEqual((await readdir(join(directory, "dist/linux"))).sort(), ["Codey-1.2.3-libo.1-linux-x64.deb", "Codey-1.2.3-libo.1-linux-x64.rpm", "Codey-1.2.3-libo.1-linux-x64.tar.gz"]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
