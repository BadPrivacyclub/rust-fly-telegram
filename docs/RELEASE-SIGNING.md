# Signing releases

Self-update always checks a release against `sha256sums.txt`. That catches a damaged download,
but anyone who can change the release can change the list too. A signature closes that gap.

Set it up once. Keep the private key off this repository and off the build machine.

```bash
openssl genpkey -algorithm ed25519 -out release-signing.pem
openssl pkey -in release-signing.pem -pubout -outform DER | tail -c 32 | base64 -w0 > assets/release-signing.pub
gh secret set RELEASE_SIGNING_KEY < release-signing.pem
```

Commit `assets/release-signing.pub`. From the next release on, `release.yml` publishes
`sha256sums.txt.sig`, and every build that contains the key refuses a release without a valid
signature. So sign the release that follows before relying on self-update, and do not lose the
private key: builds that contain the public key can only update from releases it signed.

The signature covers the tag and the checksum list, so an old signed release can't be
republished under a new tag. Self-update also refuses any version that isn't newer.
