Test fixtures only. `oidc-test-key.pem` is a throwaway RSA key generated for the OIDC tests
(oidc.rs, access.rs): it signs test ID tokens, and `oidc-test-jwks.json` is its public half as a
provider would publish it. It protects nothing; never use it for anything else.
