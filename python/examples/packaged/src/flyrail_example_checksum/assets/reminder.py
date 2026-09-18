import json
import sys
from pathlib import Path

request = json.load(sys.stdin)
print(
    json.dumps(
        {
            "version": 1,
            "outcome": "context",
            "text": Path(__file__).with_name("reminder.txt").read_text(encoding="utf-8").strip(),
        }
    )
)
