import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const media = await loadTypeScriptModule(new URL("../src/remote/images.ts", import.meta.url));
const urls = await loadTypeScriptModule(new URL("../src/remote/workspace-data.ts", import.meta.url));

test("remote image selection rejects excessive count, byte budget, empty and unsupported files before decoding", async () => {
  const image = { name: "fixture.png", type: "image/png", size: 1 };
  for (const files of [Array(5).fill(image), [{ ...image, size: media.MAX_IMAGE_BYTES + 1 }], [{ ...image, size: 0 }], [{ ...image, name: "fixture.svg", type: "image/svg+xml" }]]) {
    await assert.rejects(media.readImages(files, [], new AbortController().signal));
  }
  await assert.rejects(media.readImages([image], [{ name: "existing", url: "", size: media.MAX_IMAGE_BYTES }], new AbortController().signal), /总大小/);
});

test("remote attachments never turn local paths, scripts or SVG data into executable links", () => {
  for (const value of ["javascript:alert(1)", "file:///secret", "data:text/html,<script>1</script>", "data:image/svg+xml;base64,PHN2Zz4="]) assert.equal(urls.imageUrl(value), "");
  for (const value of ["javascript:alert(1)", "file:///secret", "codex://internal", "//other.invalid/x"]) assert.equal(urls.messageUrl(value), "");
  assert.equal(urls.imageUrl("data:image/png;base64,YQ=="), "data:image/png;base64,YQ==");
  assert.equal(urls.messageUrl("https://example.invalid/"), "https://example.invalid/");
});
