import json
import re
from dataclasses import dataclass
from typing import TypeAlias

from flyrail.values import freeze_value

PathParts: TypeAlias = tuple[str | int, ...]


@dataclass(frozen=True, slots=True)
class Node:
    start: int
    end: int
    value: object
    children: tuple[tuple[str | int, "Node", int], ...] = ()
    commas: tuple[int, ...] = ()


class JsonDocument:
    def __init__(self, text: str, *, comments: bool) -> None:
        self.text = text
        self.comments = comments
        self.index = 0
        self.root = self._node()
        self._space()
        if self.index != len(text):
            raise ValueError("unexpected data after JSON document")

    def _space(self) -> None:
        while self.index < len(self.text):
            if self.text[self.index] in " \t\r\n":
                self.index += 1
            elif self.comments and self.text.startswith("//", self.index):
                match = re.search(r"[\r\n]", self.text[self.index :])
                self.index = len(self.text) if match is None else self.index + match.start()
            elif self.comments and self.text.startswith("/*", self.index):
                end = self.text.find("*/", self.index + 2)
                if end < 0:
                    raise ValueError("unterminated JSONC comment")
                self.index = end + 2
            else:
                break

    def _node(self) -> Node:
        self._space()
        start = self.index
        if start == len(self.text):
            raise ValueError("missing JSON value")
        opening = self.text[start]
        if opening not in "[{":
            value, end = json.JSONDecoder().raw_decode(self.text, start)
            freeze_value(value)
            self.index = end
            return Node(start, end, value)
        self.index += 1
        closing = "}" if opening == "{" else "]"
        children: list[tuple[str | int, Node, int]] = []
        commas: list[int] = []
        mapping: dict[str, object] = {}
        array: list[object] = []
        self._space()
        while self.index < len(self.text) and self.text[self.index] != closing:
            key_start = self.index
            key: str | int
            if opening == "{":
                key_value = self._node()
                if not isinstance(key_value.value, str):
                    raise ValueError("JSON object keys must be strings")
                key = key_value.value
                if key in mapping:
                    raise ValueError("duplicate JSON object key")
                self._space()
                if self.index == len(self.text) or self.text[self.index] != ":":
                    raise ValueError("missing JSON object colon")
                self.index += 1
                child = self._node()
                mapping[key] = child.value
            else:
                key = len(array)
                child = self._node()
                array.append(child.value)
            children.append((key, child, key_start))
            self._space()
            if self.index == len(self.text) or self.text[self.index] != ",":
                break
            commas.append(self.index)
            self.index += 1
            self._space()
            if (
                not self.comments
                and self.index < len(self.text)
                and self.text[self.index] == closing
            ):
                raise ValueError("JSON trailing commas are unsupported")
        if self.index == len(self.text) or self.text[self.index] != closing:
            raise ValueError("unterminated or malformed JSON container")
        self.index += 1
        return Node(
            start, self.index, mapping if opening == "{" else array, tuple(children), tuple(commas)
        )

    def node(self, path: PathParts) -> Node:
        node = self.root
        for part in path:
            node = next(
                child for key, child, _ in node.children if type(key) is type(part) and key == part
            )
        return node

    def native(self, path: PathParts) -> object:
        return self.node(path).value

    def syntax(self, path: PathParts) -> bytes:
        node = self.node(path)
        return self.text[node.start : node.end].encode()

    def owned_syntax(self, path: PathParts) -> bytes:
        return self.syntax(path)

    def layout(self, path: PathParts) -> bytes:
        return b""

    def _patch(self, patches: list[tuple[int, int, str]]) -> None:
        for start, end, replacement in sorted(patches, key=lambda patch: patch[:2], reverse=True):
            self.text = self.text[:start] + replacement + self.text[end:]
        parsed = JsonDocument(self.text, comments=self.comments)
        self.root = parsed.root
        self.index = parsed.index

    def put(
        self, path: PathParts, value: object, syntax: bytes | None = None, *, layout: bytes = b""
    ) -> None:
        raw = (
            syntax.decode()
            if syntax is not None
            else json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
        )
        if not path:
            node = self.root
            self._patch([(node.start, node.end, raw)])
            return
        parent = self.node(path[:-1])
        part = path[-1]
        if isinstance(part, str) and isinstance(parent.value, dict) and part not in parent.value:
            raw = json.dumps(part, ensure_ascii=False) + ":" + raw
            self._insert(parent, len(parent.children), raw)
        else:
            node = self.node(path)
            self._patch([(node.start, node.end, raw)])

    def _insert(self, parent: Node, index: int, raw: str, prefix: bytes | None = None) -> None:
        if index < len(parent.children):
            start = parent.start + 1 if index == 0 else parent.commas[index - 1] + 1
            start = self._prefixed(start, parent.children[index][2], prefix)
            self._patch([(start, start, raw + ",")])
            return
        start = parent.end - 1
        if prefix is not None:
            boundary = (
                parent.commas[-1] + 1
                if parent.commas and len(parent.commas) == len(parent.children)
                else parent.children[-1][1].end
                if parent.children
                else parent.start + 1
            )
            start = self._prefixed(boundary, start, prefix)
        patches = [(start, start, raw)]
        if parent.children and len(parent.commas) < len(parent.children):
            end = parent.children[-1][1].end
            patches.append((end, end, ","))
        elif parent.commas:
            patches[0] = (start, start, raw + ",")
        self._patch(patches)

    def _prefixed(self, start: int, end: int, prefix: bytes | None) -> int:
        raw = (prefix or b"").decode()
        if not self.text[start:end].startswith(raw):
            raise ValueError("original array position trivia changed")
        return start + len(raw)

    def position_prefix(self, path: PathParts) -> bytes:
        parent = self.node(path[:-1])
        index = path[-1]
        if not isinstance(index, int):
            raise ValueError("array position requires a member")
        start = parent.start + 1 if index == 0 else parent.commas[index - 1] + 1
        return self.text[start : self.node(path).start].encode()

    def insert(
        self,
        path: PathParts,
        index: int,
        value: object,
        syntax: bytes | None = None,
        *,
        prefix: bytes | None = None,
        layout: bytes = b"",
    ) -> None:
        raw = (
            syntax.decode()
            if syntax is not None
            else json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
        )
        self._insert(self.node(path), index, raw, prefix)

    def delete(self, path: PathParts) -> None:
        parent = self.node(path[:-1])
        index, (_, node, start) = next(
            (index, entry) for index, entry in enumerate(parent.children) if entry[0] == path[-1]
        )
        patches = [(node.start, node.end, "")]
        if isinstance(path[-1], str):
            _, key_end = json.JSONDecoder().raw_decode(self.text, start)
            self.index = key_end
            self._space()
            patches.extend([(start, key_end, ""), (self.index, self.index + 1, "")])
        if index < len(parent.commas):
            comma = parent.commas[index]
            patches.append((comma, comma + 1, ""))
        elif parent.commas:
            comma = parent.commas[-1]
            patches.append((comma, comma + 1, ""))
        self._patch(patches)

    def relocate(
        self,
        path: PathParts,
        index: int,
        value: object,
        syntax: bytes | None = None,
        *,
        prefix: bytes | None = None,
        layout: bytes = b"",
    ) -> None:
        self.delete(path)
        self.insert(path[:-1], index, value, syntax, prefix=prefix, layout=layout)

    def empty_syntax(self, path: PathParts) -> bytes:
        return self.syntax(path)
