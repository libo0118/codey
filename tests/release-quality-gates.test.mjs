import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const workflow = fs.readFileSync(
  new URL("../.github/workflows/build-desktop.yml", import.meta.url),
  "utf8",
);
const ciWorkflow = fs.readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const macBuildScript = fs.readFileSync(
  new URL("../scripts/build.mjs", import.meta.url),
  "utf8",
);
const updateSource = fs.readFileSync(
  new URL("../backend/src/commands/updates.rs", import.meta.url),
  "utf8",
);
const windowsInstallerScript = fs.readFileSync(
  new URL("../scripts/installer/windows/Codey.nsi", import.meta.url),
  "utf8",
);

function assertRustQualityGates(job) {
  assert.match(job, /components: rustfmt, clippy/);
  assert.match(job, /cargo fmt --all -- --check/);
  assert.match(job, /cargo test --workspace --locked/);
  assert.match(job, /cargo clippy --workspace --all-targets --locked -- -D warnings/);
}

function workflowStep(from, to) {
  const fromIndex = workflow.indexOf(from);
  assert.notEqual(fromIndex, -1);
  const toIndex = workflow.indexOf(to, fromIndex);
  assert.notEqual(toIndex, -1);
  return workflow.slice(fromIndex, toIndex);
}

test("pull requests enforce the unified Rust quality gate", () => {
  assert.match(ciWorkflow, /^\s*RUSTFLAGS: -D warnings$/m);
  assertRustQualityGates(ciWorkflow);
  assert.match(ciWorkflow, /runs-on: windows-latest/);
  assert.doesNotMatch(ciWorkflow, /runs-on: (?:ubuntu|macos)/);
  assert.doesNotMatch(ciWorkflow, /tests\/overlay-recovery-native\.ps1/);
  assert.match(ciWorkflow, /working-directory: vendor\/ComputerUse\/windows\s+run: go test \.\/\.\.\./);
});

test("tag-triggered Windows releases independently enforce Rust quality gates", () => {
  assert.match(workflow, /^\s*RUSTFLAGS: -D warnings$/m);
  const windowsJob = workflow.slice(workflow.indexOf("\n  windows:"), workflow.indexOf("\n  linux:"));
  assertRustQualityGates(windowsJob);
  assert.match(windowsJob, /pnpm run check/);
  assert.match(windowsJob, /pnpm run test:js/);
  assert.doesNotMatch(windowsJob, /overlay-recovery-native\.ps1/);
  assert.match(windowsJob, /working-directory: vendor\/ComputerUse\/windows\s+run: go test \.\/\.\.\./);
  assert.match(workflow, /runs-on: windows-latest/);
  assert.doesNotMatch(windowsJob, /runs-on: (?:ubuntu|macos)/);
  assert.match(workflow, /CODEY_UPDATE_BASE_URL: https:\/\/github\.com\/libo0118\/codey\/releases\/latest\/download/);
  assert.match(workflow, /files: release-assets\/\*/);
  assert.doesNotMatch(workflow, /CLOUDFLARE|CODEY_RELEASE_ADMIN_URL|RELEASE_ADMIN_CALLBACK_SECRET/);
  assert.match(workflow, /import \{ releaseIdentity \} from/);
  assert.match(workflow, /"make_latest=" \+ identity.makeLatest/);
  assert.match(workflow, /make_latest:.*outputs.make_latest/);
  assert.match(workflow, /- name: Publish desktop release\s+if: github.event_name == 'push' && startsWith\(github.ref, 'refs\/tags\/v'\)/);
  assert.match(workflow, /prerelease:.*outputs.prerelease == 'true'/);
});

test("Linux x64 independently passes the same gates before both platform packages are assembled", () => {
  const linuxJob = workflow.slice(workflow.indexOf("\n  linux:"), workflow.indexOf("\n  release:"));
  const linuxBuildStep = workflowStep(
    "- name: Check and build Linux executables on glibc 2.28",
    "- name: Package and verify Linux tar, Debian, and RPM packages",
  );
  const releaseJob = workflow.slice(workflow.indexOf("\n  release:"));
  assert.match(linuxJob, /runs-on: ubuntu-24\.04/);
  assert.match(linuxJob, /apt-get install -y --no-install-recommends build-essential pkg-config/);
  assert.match(linuxJob, /dpkg-dev rpm cpio/);
  assert.match(linuxJob, /CODEY_REQUIRE_LINUX_PACKAGING_TOOLS: "1"/);
  assertRustQualityGates(linuxJob);
  for (const command of ["pnpm run check", "pnpm run test:js", "pnpm run vite:build", "node scripts/build.mjs", "node scripts/package-linux.mjs"]) assert.ok(linuxJob.includes(command), command);
  const linuxBuilder = fs.readFileSync(new URL("../scripts/installer/linux/Dockerfile", import.meta.url), "utf8");
  assert.match(linuxBuilder, /^FROM registry\.access\.redhat\.com\/ubi8\/ubi:8\.10$/m);
  assert.match(linuxJob, /docker build --platform linux\/amd64/);
  assert.match(linuxBuildStep, /CODEY_SKIP_OVERLAY_BUILD: "1"/);
  assert.match(linuxBuildStep, /--env CODEY_SKIP_OVERLAY_BUILD --env CODEY_UPDATE_BASE_URL/);
  assert.match(linuxBuildStep, /CARGO_TARGET_DIR=target\/linux-el8 --env CARGO_BUILD_BUILD_DIR=target\/linux-el8\/build/);
  assert.match(linuxJob, /key: linux-el8-x64\s+workspaces: "\. -> target\/linux-el8"/);
  assert.match(linuxBuildStep, /codey-linux-x64-builder bash -euo pipefail -c '[\s\S]*?cargo fmt --all -- --check[\s\S]*?cargo test --workspace --locked[\s\S]*?cargo clippy --workspace --all-targets --locked -- -D warnings[\s\S]*?node scripts\/build\.mjs[\s\S]*?ldd "target\/linux-el8\/release\/\$binary"/);
  assert.match(macBuildScript, /"cargo",\s+\[\s+"build",\s+"--release",\s+"--locked",/);
  assert.match(linuxJob, /debian:11 bash[\s\S]*?apt-get install[^\n]*linux-x64\.deb[\s\S]*?dpkg --remove codey/);
  assert.match(linuxJob, /registry\.access\.redhat\.com\/ubi8\/ubi:8\.10 bash[\s\S]*?dnf install[^\n]*linux-x64\.rpm[\s\S]*?rpm --erase codey/);
  assert.match(linuxJob, /name: codey-linux-x64-package/);
  assert.match(releaseJob, /needs: \[windows, linux\]/);
  assert.doesNotMatch(releaseJob, /always\(\)|continue-on-error/);
  assert.match(releaseJob, /name: codey-windows-x64-installer/);
  assert.match(releaseJob, /name: codey-linux-x64-package/);
  assert.match(releaseJob, /generate-update-manifest\.mjs.*windows-x64-setup\.exe.*linux-x64\.tar\.gz/);
  assert.match(releaseJob, /generate-update-manifest\.mjs.*linux-x64\.deb.*linux-x64\.rpm/);
  for (const format of ["tar.gz", "deb", "rpm"]) assert.ok(linuxJob.includes(`dist/linux/*-linux-x64.${format}`));
  assert.equal((workflow.match(/uses: softprops\/action-gh-release@/g) || []).length, 1);
  assert.doesNotMatch(workflow, /runs-on: macos|arm64|aarch64/);
});

test("Windows release reuses the prebuilt frontend overlay", () => {
  assert.match(workflow, /- name: Build embedded frontend assets\s+run: pnpm run vite:build/);
  assert.match(workflow, /cargo test --workspace --locked\s+env:\s+CODEY_SKIP_OVERLAY_BUILD: "1"/);
  assert.match(workflow, /cargo clippy --workspace --all-targets --locked -- -D warnings\s+env:\s+CODEY_SKIP_OVERLAY_BUILD: "1"/);
  assert.match(workflow, /- name: Build Windows executable\s+env:\s+CODEY_SKIP_OVERLAY_BUILD: "1"/);
  assert.match(macBuildScript, /CODEY_SKIP_OVERLAY_BUILD/);
});

test("local releases run the same locked Rust checks", () => {
  const releaseScript = fs.readFileSync(new URL("../scripts/release.mjs", import.meta.url), "utf8");
  assert.match(releaseScript, /\["fmt", "--all", "--", "--check"\]/);
  assert.match(releaseScript, /\["test", "--workspace", "--locked"\]/);
  assert.match(releaseScript, /\["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"\]/);
  assert.match(releaseScript, /\["update", "--workspace"\]/);
  assert.doesNotMatch(releaseScript, /generate-lockfile/);
});

test("desktop builds generate embedded overlay assets before Cargo compiles", () => {
  const overlayBuild = macBuildScript.indexOf("build-overlay.mjs");
  const cargoBuild = macBuildScript.indexOf('"cargo"');
  assert.notEqual(overlayBuild, -1);
  assert.notEqual(cargoBuild, -1);
  assert.ok(
    overlayBuild < cargoBuild,
    "the ignored dist-overlay directory must be generated before include_str! is compiled",
  );
});

test("macOS updates retain a rollback bundle until the replacement launches", () => {
  const backup = updateSource.indexOf('/bin/mv "$app_bundle" "$backup_bundle"');
  const install = updateSource.indexOf('/bin/mv "$tmp_dir/$app_name" "$app_bundle"');
  const launch = updateSource.indexOf('/usr/bin/open "$app_bundle"');
  const commit = updateSource.indexOf("replacement_committed=1");
  assert.ok(backup >= 0 && backup < install);
  assert.ok(install < launch && launch < commit);
  assert.match(
    updateSource,
    /if \[ "\$replacement_committed" -ne 1 \][\s\S]*?\/bin\/mv "\$backup_bundle" "\$app_bundle" \|\| true/,
  );
  assert.doesNotMatch(updateSource, /rm -rf "\$app_bundle"\s*\nmv "\$tmp_dir/);
});

test("desktop packages include FastCtx license and notice files", () => {
  for (const expected of [
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "licenses/FastCtx/LICENSE-APACHE",
    "licenses/FastCtx/NOTICE",
  ]) {
    assert.match(macBuildScript, new RegExp(expected.replaceAll("/", "\\/")));
  }

  assert.match(windowsInstallerScript, /licenses\\FastCtx\\LICENSE-APACHE/);
  assert.match(windowsInstallerScript, /licenses\\FastCtx\\NOTICE/);
});

test("Windows release publishes the installer without a portable zip", () => {
  const nsisInstallStep = workflowStep(
    "- name: Install NSIS",
    "- name: Install frontend dependencies",
  );
  const windowsPackageStep = workflowStep(
    "- name: Build Windows installer",
    "- name: Upload Windows package",
  );

  assert.match(workflow, /name: codey-windows-x64-installer/);
  assert.match(workflow, /windows-x64-setup\.exe/);
  assert.match(nsisInstallStep, /nsis-\$version\.zip/);
  assert.match(nsisInstallStep, /nsis-\$version\/nsis-\$version\.zip/);
  assert.match(
    nsisInstallStep,
    /c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1/,
  );
  assert.match(nsisInstallStep, /curl\.exe -fL --retry 3 --retry-all-errors/);
  assert.match(nsisInstallStep, /NSIS archive hash mismatch/);
  assert.match(nsisInstallStep, /Join-Path \$nsisHome "Bin" "makensis\.exe"/);
  assert.match(nsisInstallStep, /MAKENSIS=/);
  assert.doesNotMatch(nsisInstallStep, /choco install nsis/);
  assert.match(
    windowsPackageStep,
    /New-Item -ItemType Directory -Force dist\/windows \| Out-Null/,
  );
  assert.ok(
    windowsPackageStep.indexOf('New-Item -ItemType Directory -Force dist/windows') <
      windowsPackageStep.indexOf("& $env:MAKENSIS"),
  );
  assert.match(windowsPackageStep, /if \(\$LASTEXITCODE -ne 0\) \{ exit \$LASTEXITCODE \}/);
  assert.doesNotMatch(windowsPackageStep, /generate-update-manifest\.mjs/);
  assert.doesNotMatch(windowsPackageStep, /Compress-Archive|portable\.zip/);
});
