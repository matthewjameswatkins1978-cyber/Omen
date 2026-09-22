"""Async omen-shell usage with concurrent executions."""

import asyncio

from omen_shell import AsyncOmen


async def main() -> None:
    async with AsyncOmen(workspace=".") as omen:
        results = await asyncio.gather(
            *[omen.execute(["python", "-c", f"print({i})"]) for i in range(4)]
        )
        for result in results:
            print(result.execution_id, result.stdout_preview.strip())


if __name__ == "__main__":
    asyncio.run(main())
