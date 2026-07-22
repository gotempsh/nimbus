# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| latest `main` / most recent release | ✅ |
| older releases | ❌ |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Instead, report them privately to dviejo@kfs.es (or via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability),
if enabled on this repo).

Include:

- A description of the vulnerability and its potential impact
- Steps to reproduce (a minimal proof of concept, if possible)
- The affected version(s)

Note that nimbus handles cloud provider credentials passed in by the caller:
it never persists them, but bugs that could leak a credential into logs,
error messages, or request URLs are considered security issues — report
those privately too.

We aim to acknowledge reports within 3 business days and to disclose fixed
vulnerabilities via a security advisory once a patch is released.
