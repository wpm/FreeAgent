"""Typer command line interface for the Free Agent worker."""

import typer

app = typer.Typer(help="Free Agent worker.")


@app.command()
def run() -> None:
    """Run the worker."""
    typer.echo("Free Agent worker running.")


if __name__ == "__main__":
    app()
