from pathlib import Path

FIXTURES = Path(__file__).resolve().parents[1] / "spec/fixtures"
if not FIXTURES.is_dir():
    FIXTURES = Path(__file__).resolve().parents[2] / "spec/fixtures"
