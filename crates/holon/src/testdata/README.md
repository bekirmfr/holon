Test fixtures only. `oidc-test-key.pem` is a throwaway RSA key generated for the OIDC tests
(oidc.rs, access.rs): it signs test ID tokens, and `oidc-test-jwks.json` is its public half as a
provider would publish it. It protects nothing; never use it for anything else.

`jwt-test-ed25519.pem` is a throwaway Ed25519 key generated for the jwt-api sample's test
(tests.rs): it signs a partner's test tokens, and `jwt-test-ed25519.pub.pem` is its public half,
the key the test trusts. It too protects nothing.
