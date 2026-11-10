# Security policy

Volt runs with administrator rights and modifies system state, so security reports are taken
seriously.

## Supported versions

Volt is pre-alpha. Only the latest commit on `main` is supported.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository (Security tab, "Report a
vulnerability"). Please do not open a public issue.

Include what you found, how to reproduce it and the impact you expect. You will get an
acknowledgement, and a fix or a mitigation plan will be discussed with you before any public
disclosure.

## Scope

In scope: privilege escalation through Volt, unsafe handling of paths or registry keys (for
example deleting outside a rule's root through junctions or symlinks), tampering with the undo
journal, and anything that makes Volt run attacker-controlled code.

Out of scope: issues that require an attacker who already has administrator rights on the machine.
