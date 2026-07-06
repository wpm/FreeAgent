# Free Agent

_Real time multi-agent framework_

Start with:
```shell
uv run start
```

Stop with:
```shell
uv run stop
```

## Testing

Run the whole suite with:
```shell
uv run pytest
```

Tests come in two tiers:
unit tests, which need no external services, and integration tests, which run against a real
`nats-server` subprocess started by the test fixtures on a random free port. Install the binary once
for local integration runs:
```shell
brew install nats-server
```

To run only the unit tests — no NATS binary required — deselect the integration marker:
```shell
uv run pytest -m "not integration"
```