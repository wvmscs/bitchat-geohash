# Test TLS material (lot 13 bis)

Frozen files for `tests/tls.rs`. **Test only: never trust `ca.pem` outside the tests.**

| File | Content |
|---|---|
| `ca.pem` | Test certificate authority (ECDSA P-256, 100 years). Its key was deleted after signing. |
| `cert.pem` | Server certificate signed by `ca.pem`: `localhost`, `127.0.0.1`, `::1`. |
| `key.pem` | Private key of `cert.pem` (PKCS#8). Throwaway, used by the tests only. |
| `other-key.pem` | An unrelated key, for the key/certificate mismatch test. |

Made on 27/10/2026 with OpenSSL, from `ca.cnf` and `leaf.cnf`:

```sh
openssl ecparam -name prime256v1 -genkey -noout -out ca-key.pem
openssl req -new -x509 -key ca-key.pem -out ca.pem -days 36500 -config ca.cnf -extensions ext -sha256
openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out key.pem
openssl req -new -key key.pem -out leaf.csr -config leaf.cnf
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial -out cert.pem \
    -days 36500 -extfile leaf.cnf -extensions ext -sha256
openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out other-key.pem
rm ca-key.pem leaf.csr ca.srl
```

For real use, make a certificate trusted by the browser with `mkcert` (see `AGENTS.md`).
