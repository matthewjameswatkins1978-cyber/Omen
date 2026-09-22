"""Machine Contract compatibility gate.

Compatibility comes from the Machine Contract Omen reports at runtime,
not from matching version numbers. A changed contract *digest* within
a supported contract version is diagnostic evidence (cache
invalidation), not incompatibility — capability discovery decides.
"""

from __future__ import annotations

from .errors import OmenCompatibilityError

SUPPORTED_MACHINE_CONTRACTS: frozenset[str] = frozenset({"0.8"})


def check_contract(
    contract_version: str | None,
    *,
    runtime_version: str | None = None,
) -> str:
    """Raise :class:`OmenCompatibilityError` unless supported.

    Returns the contract version unchanged when supported.
    """
    if contract_version in SUPPORTED_MACHINE_CONTRACTS:
        return str(contract_version)
    raise OmenCompatibilityError(
        f"unsupported Omen Machine Contract {contract_version!r}; "
        f"supported: {sorted(SUPPORTED_MACHINE_CONTRACTS)} "
        f"(runtime {runtime_version!r})",
        found_contract=contract_version,
        supported_contracts=SUPPORTED_MACHINE_CONTRACTS,
        runtime_version=runtime_version,
    )
