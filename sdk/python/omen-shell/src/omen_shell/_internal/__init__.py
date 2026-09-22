"""Reserved private namespace.

Nothing under ``omen_shell._internal`` is part of the public API
contract. Today the transport, protocol, and compatibility machinery
lives in top-level private-by-convention modules (``transport``,
``protocol``, ``compatibility``); future refactors may move them here.
Do not import from this namespace.
"""
