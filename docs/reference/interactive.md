# Interactive Action Reference

Semantic actions begin with colon.

This list is checked against crates/omen-interactive/src/actions.rs on the 0.8 development branch.

## State and inspection

    :status [@service.<name>]
    :doctor
    :tools
    :inspect <@reference | tool-id>
    :show [@last | @failed]
    :why <@reference | fact-uri>
    :history
    :rerun [@last | @failed]

## Services

    :services
    :status @service.<name>
    :stop <@service.<name> | name>

## Agent provider

    :agent status
    :agent providers
    :agent use <provider-id>

## Execution backend

    :backend status
    :backend list
    :backend use <backend-id>

## Semantic code intelligence

    :symbol <query>
    :def <symbol-name>
    :refs <symbol-name>
    :structure <pattern> [language]
    :packages
    :tasks

## AI reasoning

AI reasoning is a separate lane:

    ? <question>

## Reference generation direction

This file should eventually be generated/validated from the canonical action registry. Do not document an action merely because a design note mentions it.
