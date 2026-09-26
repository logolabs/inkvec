# SonarQube for Inkvec (local, Community Build)

One scan that shows everything the audit tools find: Sonar's own Rust and TypeScript
analysis, plus clippy (pedantic + nursery), complexity hotspots, surviving mutants, unused
dependencies, cargo-deny findings, duplication, CodeQL and Semgrep, with line coverage.
SonarQube Community Build, Semgrep CE and the cargo tools are free. The CodeQL CLI's terms
allow it on open-source codebases (Inkvec is Apache-2.0); running it on closed-source code
(e.g. a private fork or other LogoLabs products) needs GitHub Advanced Security, so check
before reusing this setup elsewhere.

Status on 2026-09-26: the import files are generated and validated
(`target-sonar/VALIDATION.txt`), but SonarQube itself has not been run, because Docker
Desktop's engine was not started on this machine. Everything below is ready to paste.

## What goes in, and through which door

| Source | Produced by | Sonar property | File |
|---|---|---|---|
| Rust analysis (Sonar's own rules) | the scanner | built in (`sonar.rust.*`) | - |
| Line coverage | `cargo llvm-cov` | `sonar.rust.lcov.reportPaths` | `target-sonar/lcov.docker.info` |
| clippy default + pedantic + nursery | `cargo clippy --message-format=json` | `sonar.externalIssuesReportPaths` | `clippy.generic.json` |
| Cognitive complexity > 15 per function | rust-code-analysis-cli | same | `complexity.generic.json` |
| Surviving mutants | cargo-mutants | same | `mutants.generic.json` |
| Unused dependencies | cargo-machete (verified) | same | `machete.generic.json` |
| Licences, duplicate versions, advisories | cargo-deny | same | `deny.generic.json` |
| Duplicated blocks (Rust + TS) | jscpd | same | `jscpd.generic.json` |
| CodeQL Rust security + quality suites | CodeQL CLI 2.27 | `sonar.sarifReportPaths` | `codeql.sarif` |
| Semgrep `p/rust` + Inkvec rules | Semgrep CE | same | `semgrep.sarif` |

Property names were checked against Sonar's documentation on 2026-09-26:

- Rust analyzer: `sonar.rust.clippy.enabled`, `sonar.rust.cargo.manifestPaths`
  (docs.sonarsource.com/sonarqube-community-build/analyzing-source-code/languages/rust/).
- Rust coverage: `sonar.rust.lcov.reportPaths` and `sonar.rust.cobertura.reportPaths`
  (test-coverage-parameters page, Rust section).
- Generic issues: `sonar.externalIssuesReportPaths`, 10.3+ format with `rules[]`
  (`id`, `name`, `description`, `engineId`, `cleanCodeAttribute`, `impacts[]`) and
  `issues[]` (`ruleId`, `effortMinutes`, `primaryLocation{message, filePath, textRange}`,
  `secondaryLocations`). The deprecated flat format (`engineId`/`ruleId`/`severity`/`type`
  on each issue) is not used.
- SARIF: `sonar.sarifReportPaths`, SARIF 2.1.0 only; `version`, `tool.driver.name`,
  `ruleId` and `message.text` are mandatory.
- Clippy report import: documented as `sonar.rust.clippy.reportPaths`, but the analyzer
  reads `sonar.rust.clippyReport.reportPaths`, and it resolves span paths relative to each
  crate rather than the workspace root, so workspace reports lose issues (Sonar Community
  thread 178187, ticket RUST-115). That is why clippy goes through the generic import here
  and the analyzer's own clippy run is switched off (it would also need cargo in the
  scanner image).

## 1. Produce the import files (host, no Docker needed)

```powershell
(Get-Process -Id $PID).PriorityClass = 'BelowNormal'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cd <repo root>
# cheap steps + the Sonar export (about 15 min on a quiet machine; coverage is most of it)
python tools/quality_audit/run.py --out ..\out\quality-audit-$(Get-Date -f yyyy-MM-dd)
# everything, including mutation testing (~100 min), CodeQL (~40 min) and Semgrep:
python tools/quality_audit/run.py --out ..\out\qa --mutants --mutants-tmp M:\qa-audit\tmp `
    --codeql M:\qa-audit\codeql\codeql.exe --semgrep M:\qa-audit\venv\Scripts\semgrep.exe
```

This writes `target-sonar/` in the repository (ignored by git through `target-*/`) and
checks every generic file against the schema; problems are listed in
`target-sonar/VALIDATION.txt` (it must say `OK`). To re-check by hand:

```powershell
python tools/quality_audit/sonar_export.py --validate target-sonar/*.json target-sonar/*.sarif
```

Tool installs, once: `cargo binstall rust-code-analysis-cli cargo-modules cargo-llvm-cov
cargo-mutants cargo-machete cargo-deny cargo-bloat`, `rustup component add
llvm-tools-preview`, Node (for `npx jscpd`), the CodeQL bundle from
github.com/github/codeql-action/releases (unpack to a path **without spaces**: the Rust
extractor splits its argument file on spaces), and `pip install semgrep` in a venv.

## 2. Start SonarQube (once)

Docker Desktop must be running. Elasticsearch inside SonarQube needs a larger mmap limit in
Docker Desktop's WSL2 VM (reset on every Docker restart):

```powershell
wsl -d docker-desktop sysctl -w vm.max_map_count=262144
docker volume create sonarqube_data; docker volume create sonarqube_extensions; docker volume create sonarqube_logs
docker run -d --name sonarqube -p 9000:9000 --memory 6g `
  -e SONAR_SEARCH_JAVAADDITIONALOPTS="-Xms1g -Xmx1g" `
  -e SONAR_WEB_JAVAADDITIONALOPTS="-Xmx1g" -e SONAR_CE_JAVAOPTS="-Xmx2g" `
  -v sonarqube_data:/opt/sonarqube/data -v sonarqube_extensions:/opt/sonarqube/extensions `
  -v sonarqube_logs:/opt/sonarqube/logs sonarqube:community
# wait for "SonarQube is operational"
docker logs -f sonarqube
```

The Rust analyzer ships inside recent Community Builds; confirm under
Administration > Configuration > General Settings > Languages that "Rust" is listed. If it
is not, the image is too old: `docker pull sonarqube:community` and recreate the container.

## 3. Create the project and a token (once)

Browse to http://localhost:9000, log in as admin/admin and set a new password. Then, in
PowerShell (or use the UI: Create project > Local project, and My Account > Security):

```powershell
$pw = Read-Host "new admin password"     # run this yourself; agents cannot answer prompts
$auth = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("admin:$pw"))
Invoke-RestMethod -Method Post -Headers @{Authorization=$auth} "http://localhost:9000/api/projects/create?project=inkvec&name=Inkvec"
$tok = Invoke-RestMethod -Method Post -Headers @{Authorization=$auth} "http://localhost:9000/api/user_tokens/generate?name=inkvec-local&type=PROJECT_ANALYSIS_TOKEN&projectKey=inkvec"
$tok.token   # keep it; it is shown once
```

## 4. Scan

The scanner runs in Docker with the repository mounted at `/usr/src` (which is why the
LCOV file used is `lcov.docker.info`, whose paths start with `/usr/src/`):

```powershell
$repo = "M:\AI STORAGE\SVGIfication"          # or a worktree
docker run --rm -e SONAR_HOST_URL=http://host.docker.internal:9000 -e SONAR_TOKEN=$($tok.token) `
  -v "${repo}:/usr/src" sonarsource/sonar-scanner-cli `
  -Dproject.settings=tools/quality_audit/sonar-project.properties
```

Results: http://localhost:9000/dashboard?id=inkvec. External issues appear under Issues
with their engine (`clippy`, `rust-code-analysis`, `cargo-mutants`, `cargo-machete`,
`cargo-deny`, `jscpd`, and the SARIF tools by driver name).

Known limits of the imports:

- Generic issues are line-granular (no columns): Sonar rejects a whole report on one
  out-of-range column, and line precision is what these tools are good for anyway.
- Issues are only kept on indexed files. cargo-machete and cargo-deny findings belong to a
  `Cargo.toml` / `Cargo.lock`, which Sonar does not index, so they are anchored on the
  crate's `src/lib.rs` (or `main.rs`) line 1, with the manifest line in the message.
- Imported rules cannot be edited or deactivated in Sonar's quality profiles; filter them
  in the tool (e.g. `--cognitive-threshold`, clippy lint levels) or with issue exclusions.
- Semgrep's Rust grammar cannot match attributes, so the Inkvec test-only rules recognise
  test modules by name (`mod tests` and the listed `*_tests`); see the rule file.

## What this covers of Qodana (for Rust), and what it does not

JetBrains Qodana for Rust needs a paid licence. This setup reproduces most of its value:

| Qodana capability | Here |
|---|---|
| JetBrains Rust inspections | Partly: clippy (default + pedantic + nursery) covers most lint-style inspections; see RustRover below for the rest |
| Security (taint) analysis | CodeQL Rust security suite + Semgrep `p/rust` |
| Code coverage | cargo-llvm-cov (line + region) imported into Sonar |
| Duplicates | Sonar CPD + jscpd (Rust and TS) |
| Licence audit | cargo-deny licences (+ tools/third_party.py, already in CI) |
| Vulnerable dependencies | cargo-deny advisories (RustSec; cargo audit already in CI) |
| Quality gate / baseline | Sonar quality gate on new code; bench/quality.py ratchet in CI |
| Test effectiveness | Beyond Qodana: cargo-mutants surviving mutants as issues |

Optional, not wired: RustRover's bundled command-line inspector
(`<RustRover>\bin\inspect.bat <project> <profile.xml> <out-dir> -format json`, or
`rustrover64.exe inspect ...`) runs the IDE's Rust inspections headless; a small converter
from its JSON to the generic format (same pattern as `sonar_export.py`) would add
JetBrains' own inspections. RustRover's free licence is for non-commercial use only, so
running it on Inkvec as a LogoLabs product needs a paid RustRover (or All Products)
licence; that is why it is optional and not part of `run.py`.
