# Security Policy

This project is still under development, the primary goal at this stage is to implement features but we also take security very seriously. This document defines the policy on how to report vulnerabilities and receive updates when patches to those are released.

[Join Nervos Security Team!](mailto:careers@nervos.org)


## Reporting a vulnerability

All security bugs should be reported by sending email to [Nervos Security Team \<security@nervos.org>](mailto:security@nervos.org). Please encrypt your mail using GPG with the following public key.

* Unique ID: `Nervos Security Team <security@nervos.org>`
* Fingerprint: C3D9 CF8A 1450 D04B 997E  4E31 6CBD D93A 0C9F 6BCD
* Import from keys.openpgp.org: [0C9F6BCD](https://keys.openpgp.org/search?q=security@nervos.org)

This will deliver a message to Nervos Security Team who handle security issues. Your report will be acknowledged within 24 hours, and you'll receive a more detailed response to your email within 72 hours indicating the next steps in handling your report.

After the initial reply to your report the security team will endeavor to keep you informed of the progress being made towards a fix and full announcement.

## Disclosure process

1. Security report received and is assigned a primary handler. This person will coordinate the fix and release process. Problem is confirmed and all affected versions is determined. Code is audited to find any potential similar problems.
2. Fixes are prepared for all supported releases. These fixes are not committed to the public repository but rather held locally pending the announcement.
3. A suggested embargo date for this vulnerability is chosen. This notification will include patches for all supported versions.
4. On the embargo date, the [Nervos security mailing list](https://groups.google.com/u/0/a/nervos.org/g/security-mailing-list) is sent a copy of the announcement. The changes are pushed to the public repository. At least 6 hours after the mailing list is notified, a copy of the advisory will be published on Nervos community channels.

This process can take some time, especially when coordination is required with maintainers of other projects. Every effort will be made to handle the bug in as timely a manner as possible, however it's important that we follow the release process above to ensure that the disclosure is handled in a consistent manner.

## Receiving disclosures

If you require prior notification of vulnerabilities please subscribe to the [Nervos Security mailing list](https://groups.google.com/u/0/a/nervos.org/g/security-mailing-list). The mailing list is very low traffic, and it receives the public notifications the moment the embargo is lifted.

If you have any suggestions to improve this policy, please send an email to [Nervos Security Team](mailto:security@nervos.org).
