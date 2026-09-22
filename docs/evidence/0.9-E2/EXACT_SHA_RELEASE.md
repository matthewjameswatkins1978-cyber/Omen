# Exact-SHA release identity (Repair B)

A release candidate is identified by the exact source SHA that produced
its bytes, not merely by an equivalent source tree.

## PR merge-ref testing vs exact product-head identity

On `pull_request` events GitHub checks out `refs/pull/.../merge` — a
synthetic merge of the PR head into the base. That commit is useful for
integration testing (it proves the merged tree works), but it is NOT a
real commit on any branch and must NOT become the product identity.

Concretely for E2: the Preview 10 candidate was built from the merge ref
`f88193b...` (merge of `4bdf46c...` into `888d381...`). Same tree as the
branch head, different commit ID — and the artifact embedded the merge
SHA. Valid integration test, invalid release candidate.

## The rule

```
PRODUCT_SHA = pull_request ? github.event.pull_request.head.sha
            : workflow_dispatch ? inputs.target_sha
            : github.sha   (push)
```

Both workflows (`ci.yml`, `python-sdk.yml`) check out exactly
`PRODUCT_SHA` in every job and guard it:

- requested == resolved (`git rev-parse SHA^{commit}`),
- requested == checked-out (`git rev-parse HEAD`),
- manifest embedded SHA == requested == checked-out (candidate job),

failing otherwise (`EXACT_SHA_GUARD_FAILED`).

`github.sha` is still recorded — as `control_plane_sha` in the candidate
`provenance.json`, distinct from `requested_product_sha`,
`checked_out_product_sha`, and `embedded_git_sha`. The control plane may
be a synthetic merge SHA; the product SHAs must all equal the exact
remote review head. That distinction is healthy and explicit.

## Consequence

Preview 10 (built from a merge ref) is a rejected candidate independent
of its code contents. Preview 11 is the first candidate built from an
exact branch head under this rule.
