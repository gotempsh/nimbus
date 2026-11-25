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

Note that nimbus handles cloud provider credentials. The library only ever
receives them from its caller and never persists them. The CLI persists them
only when asked to by `nimbus login`: in the operating system's credential
store, or — on machines without one — in an owner-only file in its config
directory. Bugs that could leak a credential into command output, logs,
error messages, or request URLs, or that store one somewhere readable by
other users, are considered security issues — report those privately too.

We aim to acknowledge reports within 3 business days and to disclose fixed
vulnerabilities via a security advisory once a patch is released.
