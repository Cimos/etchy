# Security policy

## Reporting a vulnerability

Please report security issues **privately** through GitHub's
[private vulnerability reporting](https://github.com/Cimos/etchy/security/advisories/new)
(the repository's **Security → Report a vulnerability** tab). Do not open a
public issue for a suspected vulnerability.

We aim to acknowledge a report within a few days and will coordinate a fix and
disclosure timeline with you.

## Scope

etchy parses **untrusted input** — Gerber (RS-274X/X2), Excellon drill, and
PDF files. The kinds of issues most relevant here:

- crashes, unbounded memory/CPU, or hangs on crafted input (parsers carry
  per-file DoS ceilings and are fuzzed, but reports of gaps are welcome);
- a diff that silently drops or misreports a real change (a trust defect — see
  `docs/TRUST.md`);
- anything in the GitHub Action path that could be abused by a crafted input
  value in a consumer's CI.

## Supported versions

etchy is pre-1.0; only the latest `main` (and the most recent release, once
published) receives fixes.
