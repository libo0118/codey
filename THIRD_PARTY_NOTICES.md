# Third-Party Notices

Codey is licensed under the GNU Affero General Public License version 3 only.
The complete project license is in `LICENSE`.

Codey contains or depends on third-party software and data. Those components
remain under their respective licenses and copyright notices.

## Vendored source

`vendor/CodeyRuntime` is distributed as source under `AGPL-3.0-only`. Its
license text is also preserved at `vendor/CodeyRuntime/LICENSE`.

## Bundled context tool

Codey's optional built-in context tool sidecar includes FastCtx.

| Component | Locked version | License | Source or copyright |
| --- | --- | --- | --- |
| `fastctx` | 0.2.6 | Apache-2.0 | [yc-duan/fastctx](https://github.com/yc-duan/fastctx); Copyright (c) 2026 yc-duan <dy2958830371@gmail.com> |

FastCtx's Apache License 2.0 text and NOTICE file are preserved in
`licenses/FastCtx/`.

The following attribution is reproduced verbatim from FastCtx's NOTICE:

    This product includes FastCtx
    (https://github.com/yc-duan/fastctx), Copyright (c) 2026 yc-duan,
    used under the Apache License 2.0.

    FastCtx is redistributed and/or modified here by the maintainer of
    this distribution. Any such change is that maintainer's own work
    and their sole responsibility. It is not endorsed by, not
    supported by, and not attributable to the author of FastCtx, who
    accepts no liability of any kind arising from this distribution or
    from anything built on top of it.

## License references

- [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0)

## Selectively adapted integrations

The optional CodeGraph/Ponytail management, remote sessions, session retention,
and configuration backup adapt source from
[fraternity-z/CodeyX](https://github.com/fraternity-z/CodeyX/tree/30e1daa4861762626628e6c2805366cd6519e6f2),
commit `30e1daa4861762626628e6c2805366cd6519e6f2`, under `AGPL-3.0-only`.
The complete AGPL license is included in `LICENSE`.

[CodeGraph](https://github.com/colbymchenry/codegraph) is an optional,
user-enabled download under MIT; its runtime and dependencies retain their own
licenses. Runtime assets and `SHA256SUMS` come from that repository's `v1.6.2`
release. Same-source SHA-256 checks are not independent signature verification.
The runtime is not bundled with Codey.

The remote session protocol adapts
[codex-mobile-bridge](https://github.com/try2love/codex-mobile-bridge), MIT,
Copyright (c) 2026 try2love. Its original license is preserved in
`third_party/codex-mobile-bridge.LICENSE` and reproduced below so it also ships
with distributions containing this notice.

### codex-mobile-bridge MIT License

MIT License

Copyright (c) 2026 try2love

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Names and trademarks belong to their respective owners. Their inclusion does
not imply affiliation with or endorsement of Codey.
