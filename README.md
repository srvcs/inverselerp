# srvcs-inverselerp

The inverse-linear-interpolation orchestrator of the srvcs.cloud distributed
standard library.

Its single concern: **range: inverse linear interpolation (where value lies
between a and b).** It owns the *control flow* — composing two float primitives
— but does no arithmetic of its own. It asks
[`srvcs-floatsubtract`](https://github.com/srvcs/floatsubtract) for the
numerator and denominator, then
[`srvcs-floatdivide`](https://github.com/srvcs/floatdivide) for their quotient.

```
inverselerp(a, b, value):
    num = floatsubtract(value, a)   # value - a
    den = floatsubtract(b, a)       # b - a
    return floatdivide(num, den)    # (value - a) / (b - a)
```

`inverselerp(0, 10, 5) == 0.5`.

Validation is not handled here. This service never calls `srvcs-isnumber`
directly; instead its dependencies validate their own operands, and any `422`
they raise is forwarded verbatim.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Compute `inverselerp(a, b, value)` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"a": 0, "b": 10, "value": 5}'
# {"a":0,"b":10,"value":5,"result":0.5}
```

Responses:

- `200 {"a": a, "b": b, "value": value, "result": f}` — evaluated; `result` is a
  float.
- `422` — a dependency rejected the input, forwarded verbatim.
- `500` — a reachable dependency returned a `200` without a float `result`
  (a contract violation).
- `503` — a dependency is unavailable.

## Dependencies

- [`srvcs-floatsubtract`](https://github.com/srvcs/floatsubtract)
- [`srvcs-floatdivide`](https://github.com/srvcs/floatdivide)

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_FLOATSUBTRACT_URL` | `http://127.0.0.1:8090` | Base URL of `srvcs-floatsubtract` |
| `SRVCS_FLOATDIVIDE_URL` | `http://127.0.0.1:8091` | Base URL of `srvcs-floatdivide` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up *computing* mock `srvcs-floatsubtract` and
`srvcs-floatdivide` services in-process — they read the request body and return
the real `a - b` / `a / b`, so the composition is genuinely exercised against
the asserted cases (within `1e-9` tolerance). See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
