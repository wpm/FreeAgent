# Security

## Reporting a vulnerability

Please report suspected vulnerabilities privately to the maintainer rather than
opening a public issue.

## Dependency advisories

### litellm — MAL-2026-2144 / MSC-2026-3284 (false positive)

**Status: not exploitable. The alert is a known false positive raised by
dependency scanners (e.g. PyCharm's Package Checker); suppress it in the
scanner that reports it.**

`MAL-2026-2144` is the March 2026 litellm supply-chain backdoor. The malicious
code shipped in **only litellm `1.82.7` and `1.82.8`** (published by the
TeamPCP threat actor via a compromised Trivy CI dependency); PyPI quarantined
both within roughly three hours, and the clean `1.83.0` was released on
2026-03-30.

This project pins **`litellm>=1.89.0`** in
[`packages/freeagent/pyproject.toml`](packages/freeagent/pyproject.toml), and
[`uv.lock`](uv.lock) resolves litellm to **`1.89.0`** (released 2026-06-13,
sdist `sha256:eb1910a2…`, wheel `sha256:63b33e2d…`). That version is clean and
post-dates every litellm security fix to date:

| Advisory | Issue | Fixed in |
| --- | --- | --- |
| MAL-2026-2144 | Supply-chain backdoor | only 1.82.7 / 1.82.8 affected; clean since 1.83.0 |
| CVE-2026-42271 | Proxy command injection (RCE) | 1.83.7 |
| CVE-2026-30623 | Proxy MCP stdio command injection | 1.83.7 |
| CVE-2026-42208 | Proxy SQL injection | 1.83.10 |
| GHSA host-header bypass | Proxy auth bypass | 1.84.0 |

The advisory still fires because the OSV/GHSA record for MAL-2026-2144
enumerates affected versions as **`1.*`, `1.82.7`, `1.82.8` with no recorded
patched version** (see <https://osv.dev/vulnerability/MAL-2026-2144>). Scanners
that key off the over-broad `1.*` entry flag every 1.x release, including the
clean `1.89.0` — a well-known false-positive pattern for malicious-package
advisories.

Additionally, every litellm CVE above targets the **proxy server**. This
project uses litellm only as an **SDK** — a single deferred call to
`litellm.acompletion` in
[`packages/freeagent/src/freeagent/llm.py`](packages/freeagent/src/freeagent/llm.py)
— and never runs the proxy, so those CVEs do not apply regardless.

**Resolution:** keep `litellm>=1.89.0` and suppress the finding in whatever
scanner reports it, citing this file:

- **PyCharm Package Checker** (where this was first seen): right-click the
  flagged dependency and ignore the vulnerability, or disable/exclude it under
  *Settings → Editor → Inspections → Package Checker*. This is a local IDE
  setting, not committed to the repo.
- **GitHub Dependabot** (if enabled): GitHub's Advisory Database may already
  scope the malware advisory to only 1.82.7/1.82.8, in which case 1.89.0 is
  not flagged and there is nothing to dismiss. If an alert does appear, dismiss
  it in *Security → Dependabot alerts* as inaccurate / a false positive.

The `>=1.89.0` floor must never be lowered below `1.83.0`, as that would
re-admit the compromised versions.

#### Sources

- OSV MAL-2026-2144: <https://osv.dev/vulnerability/MAL-2026-2144>
- PyPI incident report:
  <https://blog.pypi.org/posts/2026-04-02-incident-report-litellm-telnyx-supply-chain-attack/>
- litellm security hardening / CVE fix versions:
  <https://docs.litellm.ai/blog/security-hardening-april-2026>
