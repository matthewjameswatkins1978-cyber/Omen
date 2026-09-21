# Omen machine semantic results

Omen exposes one semantic observation model from `omen-semantic`:
`SemanticResult<T>`. Its public schema version is `1`, independently of the
Machine Contract and the `OmenError` schema.

The envelope contains `operation`, `outcome`, `coverage`, `generation`, and
typed operation-specific `data`:

- `symbol_search` uses `SemanticSearchData { matches }`.
- `definition` uses `SemanticDefinitionData { resolved, candidates }`.
- `references` uses `SemanticReferencesData { references, candidates }`.

`FOUND`, `NOT_FOUND`, and `AMBIGUOUS` are successful observations. A search
returns a set and therefore represents multiple matches directly; definition
and references retain candidates instead of selecting an arbitrary winner.
`UNSUPPORTED`, `PROVIDER_FAILURE`, and `SEMANTIC_HINT_MISMATCH` are domain
errors and use the frozen `OmenError` contract.

Coverage is `COMPLETE`, `PARTIAL`, or `NONE`. `PARTIAL` means the result is
valid for the observable provider-backed portion of the workspace, but the
operation cannot claim universal coverage. Coverage is derived from registered
provider/resource declarations and cached workspace knowledge; it is not a
substitute for a full workspace scan on every query.

Generation records the workspace and provider observation generation. It is a
freshness witness, not permission or replay authority.

Direct MCP semantic tools and composition semantic capabilities serialize the
same `SemanticResult<T>` value. MCP adds normal tool-result and JSON-RPC
framing. Composition adds its normal execution-step envelope. Neither route
reconstructs semantic truth from prose or a second status mapper.

The existing raw `SemanticLookupResult<T>` remains an internal/provider-facing
compatibility type. It is normalized once at the registry boundary. Existing
provider APIs remain available during 0.9 migration; new public projections
must use the canonical result methods.

The governing rule is:

> A semantic result describes what Omen was able to establish, not what the
> user hoped was true.

The semantic result contract is additive to Machine Contract `0.8`; no
Machine Contract increment is required. Retryability and generation do not
imply that an operation is safe to replay.
