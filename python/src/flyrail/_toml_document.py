import json
from collections.abc import MutableMapping
from copy import deepcopy
from typing import cast

import tomlkit
from tomlkit.container import Container, OutOfOrderTableProxy
from tomlkit.exceptions import TOMLKitError
from tomlkit.items import AbstractTable, AoT, Array, Comment, Item, Null, Table, Trivia, Whitespace
from tomlkit.parser import Parser

from flyrail._json_document import PathParts
from flyrail._toml_layout import header_path, layout_headers


class TomlDocument:
    def __init__(self, text: str) -> None:
        self.document = self._coalesce(tomlkit.parse(text))
        self.newline = "\r\n" if "\r\n" in text else "\n"

    def _coalesce(self, container: Container) -> Container:
        result = type(container)(container._parsed)
        for key, node in container.body:
            if isinstance(node, Table):
                node._value = self._coalesce(node.value)
                dict.update(node, node.value)
            elif isinstance(node, AoT):
                for table in node.body:
                    table._value = self._coalesce(table.value)
                    dict.update(table, table.value)
            if result.body and key is not None and key.is_dotted() and isinstance(node, Table):
                index = len(result.body) - 1
                while index > 0 and isinstance(result.body[index][1], Comment | Whitespace):
                    index -= 1
                previous_key, previous = result.body[index]
                if (
                    previous_key is not None
                    and previous_key.as_string() == key.as_string()
                    and previous_key.is_dotted()
                    and isinstance(previous, Table)
                ):
                    for gap_key, gap in result.body[index + 1 :]:
                        previous.value._raw_append(gap_key, gap)
                    del result.body[index + 1 :]
                    for child_key, child in node.value.body:
                        previous.value._raw_append(child_key, child)
                    previous._value = self._coalesce(previous.value)
                    dict.update(previous, previous.value)
                    dict.__setitem__(result, key.key, previous.value)
                    continue
            result._raw_append(key, node)
        return result

    @property
    def text(self) -> str:
        return tomlkit.dumps(self.document)

    def _item(self, path: PathParts) -> Item | Container | OutOfOrderTableProxy:
        node: Item | Container | OutOfOrderTableProxy = self.document
        for part in path:
            if isinstance(part, str) and isinstance(
                node, Container | AbstractTable | OutOfOrderTableProxy
            ):
                node = (
                    node._internal_container.item(part)
                    if isinstance(node, OutOfOrderTableProxy)
                    else node.item(part)
                )
            elif isinstance(part, int) and isinstance(node, Array):
                node = node.item(index=part)
            elif isinstance(part, int) and isinstance(node, AoT):
                node = node[part]
            else:
                raise ValueError("TOML selector crosses a scalar")
        return node

    def native(self, path: PathParts) -> object:
        return self._item(path).unwrap()

    def _contiguous(self, path: PathParts) -> None:
        events: list[PathParts] = []

        def visit(node: Item | Container, current: PathParts) -> None:
            if isinstance(node, Table | Container):
                if isinstance(node, Table) and not node.is_super_table():
                    events.append(current)
                body = node.value.body if isinstance(node, Table) else node.body
                for key, child in body:
                    if key is not None:
                        visit(child, (*current, key.key))
            elif isinstance(node, AoT):
                for index, child in enumerate(node.body):
                    visit(child, (*current, index))
            else:
                events.append(current)

        visit(self.document, ())
        selected = [index for index, event in enumerate(events) if event[: len(path)] == path]
        if selected and selected != list(range(selected[0], selected[-1] + 1)):
            raise ValueError("a noncontiguous TOML table cannot be an owned unit")

    def _tables(self, node: Table | OutOfOrderTableProxy) -> list[Table]:
        return node._tables if isinstance(node, OutOfOrderTableProxy) else [node]

    def _copy(
        self, node: Item | Container | OutOfOrderTableProxy
    ) -> Item | Container | OutOfOrderTableProxy:
        if isinstance(node, OutOfOrderTableProxy):
            container = Container(True)
            for table in node._tables:
                container._raw_append(tomlkit.key("value"), deepcopy(table))
            return container.item("value")
        return deepcopy(node)

    def _invalidate(self, node: Item | OutOfOrderTableProxy) -> None:
        if isinstance(node, Table | OutOfOrderTableProxy):
            for table in self._tables(node):
                table.display_name = None
                for _, child in table.value.body:
                    self._invalidate(child)
        elif isinstance(node, AoT):
            for table in node.body:
                self._invalidate(table)

    def _fragment_syntax(self, node: OutOfOrderTableProxy, *, synthetic: bool) -> bytes:
        document = tomlkit.document()
        for table in node._tables:
            if not table.is_super_table():
                table.trivia.indent = ""
                table.trivia.comment_ws = ""
                table.trivia.comment = ""
                table.trivia.trail = self.newline
            if synthetic:
                self._invalidate(table)
            else:
                table.display_name = None
            document._raw_append(tomlkit.key("value"), table)
        return tomlkit.dumps(document).encode()

    def owned_syntax(self, path: PathParts) -> bytes:
        node = self._copy(self._item(path))
        if isinstance(node, Table | AoT | OutOfOrderTableProxy):
            self._contiguous(path)
        self._strip_gap(node)
        if isinstance(node, OutOfOrderTableProxy):
            return self._fragment_syntax(node, synthetic=False)
        if isinstance(node, AoT):
            document = tomlkit.document()
            document._raw_append(tomlkit.key("value"), node)
            return tomlkit.dumps(document).encode()
        return node.as_string().encode()

    def syntax(self, path: PathParts) -> bytes:
        node = self._copy(self._item(path))
        if isinstance(node, Table | AoT | OutOfOrderTableProxy):
            self._contiguous(path)
        self._strip_gap(node)
        if isinstance(node, OutOfOrderTableProxy):
            return self._fragment_syntax(node, synthetic=True)
        if not isinstance(node, Item):
            raise ValueError("a noncontiguous TOML table cannot be an owned unit")
        node.trivia.indent = ""
        node.trivia.comment_ws = ""
        node.trivia.comment = ""
        node.trivia.trail = self.newline
        self._invalidate(node)
        document = tomlkit.document()
        document["value"] = node
        return tomlkit.dumps(document).encode()

    def layout(self, path: PathParts) -> bytes:
        headers: list[tuple[PathParts, str]] = []

        def visit(node: Item | Container | OutOfOrderTableProxy, relative: PathParts) -> None:
            if isinstance(node, Table | OutOfOrderTableProxy):
                for table in self._tables(node):
                    if relative and table.display_name is not None and not table.is_super_table():
                        headers.append((relative, table.display_name))
                    for key, child in table.value.body:
                        if key is not None:
                            visit(child, (*relative, key.key))
            elif isinstance(node, AoT):
                for index, child in enumerate(node.body):
                    visit(child, (*relative, index))

        visit(self._item(path), ())
        return (
            json.dumps(
                {"version": 1, "headers": headers}, ensure_ascii=False, separators=(",", ":")
            ).encode()
            if headers
            else b""
        )

    def _trailing_container(
        self, node: Item | Container | OutOfOrderTableProxy
    ) -> Container | None:
        if isinstance(node, OutOfOrderTableProxy):
            return self._trailing_container(node._tables[-1])
        if isinstance(node, AoT) and node:
            return self._trailing_container(node[-1])
        if isinstance(node, Table):
            if node.value.body:
                child = node.value.body[-1][1]
                if isinstance(child, Table | AoT):
                    return self._trailing_container(child)
            return node.value
        return None

    def _strip_gap(self, node: Item | Container | OutOfOrderTableProxy) -> str:
        container = self._trailing_container(node)
        gap = ""
        if container is not None:
            while container.body and isinstance(container.body[-1][1], Comment | Whitespace | Null):
                gap = container.body.pop()[1].as_string() + gap
        return gap

    def _append_gap(self, node: Item | OutOfOrderTableProxy, gap: str) -> None:
        container = self._trailing_container(node)
        if gap and container is not None:
            container.append(None, tomlkit.ws(gap))

    def _restore_layout(self, path: PathParts, layout: bytes) -> None:
        for relative, name in layout_headers(layout):
            if header_path(name) != tuple(
                part for part in (*path, *relative) if isinstance(part, str)
            ):
                raise ValueError("TOML restoration header does not match its destination")
            try:
                node = self._item((*path, *relative))
            except (KeyError, IndexError) as error:
                raise ValueError("invalid TOML restoration path") from error
            if isinstance(node, OutOfOrderTableProxy):
                tables = [table for table in node._tables if not table.is_super_table()]
                if not tables:
                    raise ValueError("invalid TOML restoration header")
                node = tables[0]
            if not isinstance(node, Table):
                raise ValueError("invalid TOML restoration header")
            node.display_name = name

    def _generated(
        self, value: object, syntax: bytes | None, *, element: bool = False
    ) -> Item | OutOfOrderTableProxy:
        if syntax is not None:
            node = tomlkit.parse(syntax.decode()).item("value")
        else:
            document = tomlkit.document()
            document["value"] = value
            text = tomlkit.dumps(document).replace("\n", self.newline)
            node = tomlkit.parse(text).item("value")
        if element and isinstance(node, AoT) and len(node) == 1:
            node = node[0]
        self._invalidate(node)
        return node

    def put(
        self, path: PathParts, value: object, syntax: bytes | None = None, *, layout: bytes = b""
    ) -> None:
        self._put(path, value, syntax)
        self._restore_layout(path, layout)
        self._publish()

    def _publish(self) -> None:
        expected = self.document.unwrap()
        try:
            parsed = tomlkit.parse(self.text)
        except TOMLKitError as error:
            raise ValueError("TOML layout cannot preserve valid syntax") from error
        if self._semantic(parsed.unwrap()) != self._semantic(expected):
            raise ValueError("TOML layout cannot preserve the selected and foreign values")
        self.document = self._coalesce(parsed)

    def _semantic(self, value: object) -> object:
        if isinstance(value, dict):
            return {key: self._semantic(item) for key, item in value.items()}
        if isinstance(value, list):
            return [self._semantic(item) for item in value]
        return type(value).__name__, repr(value)

    def _dotted(self, path: PathParts) -> bool:
        if not path or not isinstance(path[-1], str):
            return False
        parent = self._item(path[:-1])
        container = parent.value if isinstance(parent, AbstractTable) else parent
        if isinstance(container, OutOfOrderTableProxy):
            container = container._internal_container
        return isinstance(container, Container) and any(
            key is not None and key.key == path[-1] and key.is_dotted() for key, _ in container.body
        )

    def _put(self, path: PathParts, value: object, syntax: bytes | None = None) -> None:
        part = path[-1]
        node = self._generated(value, syntax, element=isinstance(part, int))
        parent = self._item(path[:-1])
        current = (
            self._item(path)
            if isinstance(part, str)
            and isinstance(parent, Container | AbstractTable | OutOfOrderTableProxy)
            and part in parent
            else None
        )
        if isinstance(current, OutOfOrderTableProxy) or isinstance(node, OutOfOrderTableProxy):
            self._put_fragments(path, node, current)
            return
        if (
            isinstance(part, str)
            and isinstance(parent, Container | AbstractTable | OutOfOrderTableProxy)
            and part not in parent
            and self.text
            and not self.text.endswith(("\r", "\n"))
        ):
            self.document = self._coalesce(tomlkit.parse(self.text + self.newline))
            parent = self._item(path[:-1])
        trivia = self._trivia(self.document)
        indent = node.trivia.indent
        if isinstance(part, str) and isinstance(
            parent, Container | AbstractTable | OutOfOrderTableProxy
        ):
            mapping = cast(MutableMapping[str, object], parent)
            if part in mapping:
                current = (
                    parent._internal_container.item(part)
                    if isinstance(parent, OutOfOrderTableProxy)
                    else parent.item(part)
                )
                if isinstance(current, Table | AoT) and not isinstance(node, type(current)):
                    raise ValueError(
                        "TOML table representation changes cannot preserve foreign layout"
                    )
                if not isinstance(current, Table | AoT):
                    node = self._inline(node)
                elif isinstance(node, Table) and self._dotted(path):
                    node._is_super_table = True
                    if syntax is None:
                        for key in list(node):
                            child = node.item(key)
                            if isinstance(child, Table):
                                node[key] = self._inline(child)
                        self._strip_gap(node)
                if isinstance(current, Table) and isinstance(node, Table):
                    self._preserve_header(node, current)
                if isinstance(current, Item):
                    self._append_gap(node, self._strip_gap(deepcopy(current)))
                    indent = current.trivia.indent
                    node.trivia.indent = current.trivia.indent
                    node.trivia.comment_ws = current.trivia.comment_ws
                    node.trivia.comment = current.trivia.comment
                    node.trivia.trail = current.trivia.trail
            elif self._dotted(path[:-1]):
                node = self._inline(node)
            body_length = len(node.value.body) if isinstance(node, Table) else 0
            first_indent = node[0].trivia.indent if isinstance(node, AoT) and node else None
            mapping[part] = node
            if isinstance(node, AoT) and first_indent is not None:
                node[0].trivia.indent = first_indent
            if isinstance(node, Table):
                while len(node.value.body) > body_length and isinstance(
                    node.value.body[-1][1], Whitespace
                ):
                    node.value.body.pop()
            for item, original in trivia:
                item.trivia.indent = original.indent
                item.trivia.comment_ws = original.comment_ws
                item.trivia.comment = original.comment
                item.trivia.trail = original.trail
            node.trivia.indent = indent
        elif isinstance(part, int) and isinstance(parent, Array):
            self._array_edit(path[:-1], part, self._inline(node).as_string(), replace=True)
        elif isinstance(part, int) and isinstance(parent, AoT):
            self._append_gap(node, self._strip_gap(deepcopy(parent[part])))
            node._trivia = parent[part].trivia.copy()
            parent[part] = node
        else:
            raise ValueError("TOML selector crosses a scalar")

    def _put_fragments(
        self,
        path: PathParts,
        node: Item | OutOfOrderTableProxy,
        current: Item | Container | OutOfOrderTableProxy | None,
    ) -> None:
        if not isinstance(current, Table | OutOfOrderTableProxy) or not isinstance(
            node, Table | OutOfOrderTableProxy
        ):
            raise ValueError("TOML table representation changes cannot preserve foreign layout")
        self._contiguous(path)
        container = self._physical_parent(path)
        tables = self._tables(node)
        self._preserve_header(node, current)
        self._append_gap(node, self._strip_gap(self._copy(current)))
        body = list(container.body)
        key = next(key for key, _ in body if key is not None and key.key == path[-1])
        indices = [index for index, (part, _) in enumerate(body) if part == key]
        replacement = [(key, table) for table in tables]
        body[indices[0] : indices[-1] + 1] = replacement
        container._map.clear()
        container._body.clear()
        container._table_keys.clear()
        container._out_of_order_keys.clear()
        container._validation_cache.clear()
        dict.clear(container)
        for part, item in body:
            container._raw_append(part, item)

    def _preserve_header(
        self, node: Table | OutOfOrderTableProxy, current: Table | OutOfOrderTableProxy
    ) -> None:
        live_header = next(
            (table for table in self._tables(current) if not table.is_super_table()), None
        )
        if live_header is not None:
            tables = self._tables(node)
            header = next((table for table in tables if not table.is_super_table()), tables[0])
            header._is_super_table = False
            header._trivia = live_header.trivia.copy()
            header.display_name = live_header.display_name

    def _physical_parent(self, path: PathParts) -> Container:
        parents: list[Item | Container] = [self.document]
        for part in path[:-1]:
            children: list[Item | Container] = []
            for parent in parents:
                if isinstance(parent, Table | Container) and isinstance(part, str):
                    container = parent.value if isinstance(parent, Table) else parent
                    children.extend(
                        child
                        for key, child in container.body
                        if key is not None and key.key == part
                    )
                elif isinstance(parent, AoT) and isinstance(part, int):
                    children.append(parent[part])
            parents = children
        containers = [
            parent.value if isinstance(parent, Table) else parent
            for parent in parents
            if isinstance(parent, Table | Container) and path[-1] in parent
        ]
        if len(containers) != 1:
            raise ValueError("TOML fragments cross separate parent table representations")
        return containers[0]

    def _inline(self, node: Item) -> Item:
        if isinstance(node, Table):
            inline = tomlkit.inline_table()
            inline.update(node.unwrap())
            return inline
        return node

    def _trivia(self, node: Item | Container) -> list[tuple[Item, Trivia]]:
        result = [] if isinstance(node, Container | Whitespace) else [(node, node.trivia.copy())]
        children: list[Item] = []
        if isinstance(node, Container):
            children = [item for _, item in node.body]
        elif isinstance(node, AbstractTable):
            children = [item for _, item in node.value.body]
        elif isinstance(node, Array):
            children = [node.item(index) for index in range(len(node))]
        elif isinstance(node, AoT):
            children = list(node.body)
        for child in children:
            result.extend(self._trivia(child))
        return result

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
        parent = self._item(path)
        node = self._generated(value, syntax, element=True)
        if isinstance(node, OutOfOrderTableProxy):
            raise ValueError("TOML table fragments cannot become an array member")
        if isinstance(parent, Array):
            self._array_edit(path, index, self._inline(node).as_string(), prefix=prefix)
        elif isinstance(parent, AoT) and isinstance(node, Table):
            parent.insert(index, node)
        else:
            raise ValueError("TOML member selector requires an array")
        self._restore_layout((*path, index), layout)
        self._publish()

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
        parent = self._item(path[:-1])
        part = path[-1]
        if isinstance(parent, AoT) and isinstance(part, int):
            node = parent[part]
            del parent[part]
            parent.insert(index, node)
            self.put((*path[:-1], index), value, syntax, layout=layout)
        else:
            self.delete(path)
            self.insert(path[:-1], index, value, syntax, prefix=prefix, layout=layout)

    def _array_parts(self, path: PathParts) -> tuple[str, list[tuple[int, int]], list[int]]:
        parent = self._item(path)
        if not isinstance(parent, Array):
            raise ValueError("inline array edit requires an array")
        text = parent.as_string()
        parser = Parser(text)
        parser.inc()
        spans: list[tuple[int, int]] = []
        commas: list[int] = []
        while True:
            parser.consume(" \t\r\n")
            if parser._current == "#":
                while parser._current not in "\r\n" and not parser.end():
                    parser.inc()
            elif parser._current == ",":
                commas.append(parser._idx)
                parser.inc()
            elif parser._current == "]":
                break
            else:
                start = parser._idx
                parser._parse_value()
                spans.append((start, parser._idx))
        return text, spans, commas

    def position_prefix(self, path: PathParts) -> bytes:
        if not isinstance(self._item(path[:-1]), Array):
            return b""
        text, spans, commas = self._array_parts(path[:-1])
        index = path[-1]
        if not isinstance(index, int):
            raise ValueError("array position requires a member")
        start = 1 if index == 0 else commas[index - 1] + 1
        return text[start : spans[index][0]].encode()

    def _array_edit(
        self,
        path: PathParts,
        index: int,
        raw: str | None,
        *,
        replace: bool = False,
        prefix: bytes | None = None,
    ) -> None:
        text, spans, commas = self._array_parts(path)
        patches: list[tuple[int, int, str]] = []
        if raw is None or replace:
            start, end = spans[index]
            patches.append((start, end, raw or ""))
            if raw is None and commas:
                comma = commas[min(index, len(commas) - 1)]
                patches.append((comma, comma + 1, ""))
        elif index < len(spans):
            start = 1 if index == 0 else commas[index - 1] + 1
            start = self._prefixed(text, start, spans[index][0], prefix)
            patches.append((start, start, raw + ","))
        else:
            start = len(text) - 1
            if prefix is not None:
                boundary = (
                    commas[-1] + 1
                    if commas and len(commas) == len(spans)
                    else spans[-1][1]
                    if spans
                    else 1
                )
                start = self._prefixed(text, boundary, start, prefix)
            patches.append((start, start, raw))
            if spans and len(commas) < len(spans):
                end = spans[-1][1]
                patches.append((end, end, ","))
            elif commas:
                patches[0] = (start, start, raw + ",")
        for start, end, replacement in sorted(patches, key=lambda patch: patch[:2], reverse=True):
            text = text[:start] + replacement + text[end:]
        self.put(path, None, ("value = " + text).encode())

    def _prefixed(self, text: str, start: int, end: int, prefix: bytes | None) -> int:
        raw = (prefix or b"").decode()
        if not text[start:end].startswith(raw):
            raise ValueError("original array position trivia changed")
        return start + len(raw)

    def delete(self, path: PathParts) -> None:
        parent = self._item(path[:-1])
        part = path[-1]
        if isinstance(part, int) and isinstance(parent, Array):
            self._array_edit(path[:-1], part, None)
        elif isinstance(part, int) and isinstance(parent, AoT):
            table = parent[part]
            comment = self._header_comment(table) + self._strip_gap(deepcopy(table))
            del parent[part]
            if comment:
                if part < len(parent):
                    parent[part].trivia.indent = comment + parent[part].trivia.indent
                elif parent:
                    parent[-1].value.append(None, tomlkit.ws(comment))
            if not parent:
                self._remove_empty_aot(path[:-1], comment)
        elif isinstance(part, str) and isinstance(
            parent, Container | AbstractTable | OutOfOrderTableProxy
        ):
            node = (
                parent._internal_container.item(part)
                if isinstance(parent, OutOfOrderTableProxy)
                else parent.item(part)
            )
            if isinstance(node, OutOfOrderTableProxy):
                parent = self._physical_parent(path)
            body = (
                parent.value.body
                if isinstance(parent, AbstractTable)
                else parent.body
                if isinstance(parent, Container)
                else None
            )
            retained_gap: tuple[int, str] | None = None
            if isinstance(node, Item | OutOfOrderTableProxy):
                gap = self._strip_gap(self._copy(node))
                header = (
                    next(
                        (table for table in node._tables if not table.is_super_table()),
                        node._tables[0],
                    )
                    if isinstance(node, OutOfOrderTableProxy)
                    else node
                )
                if header.trivia.comment:
                    trivia = header.trivia
                    gap = trivia.indent + trivia.comment_ws + trivia.comment + trivia.trail + gap
                if gap:
                    if body is None:
                        raise ValueError("cannot safely detach this TOML comment")
                    first = node._tables[0] if isinstance(node, OutOfOrderTableProxy) else node
                    index = next(index for index, (_, item) in enumerate(body) if item is first)
                    retained_gap = index, gap
            del cast(MutableMapping[str, object], parent)[part]
            if retained_gap is not None and body is not None:
                index, gap = retained_gap
                body[index] = (None, tomlkit.ws(gap))
            self._retain_empty_tables(self.document)
        else:
            raise ValueError("TOML selector crosses a scalar")
        self._publish()

    def _retain_empty_tables(self, container: Container) -> None:
        for _, node in container.body:
            if isinstance(node, Table):
                if not node.value and node.is_super_table():
                    node._is_super_table = False
                    node._trivia = Trivia(trail=self.newline)
                self._retain_empty_tables(node.value)

    def _header_comment(self, node: Table) -> str:
        trivia = node.trivia
        return (
            trivia.indent + trivia.comment_ws + trivia.comment + trivia.trail
            if trivia.comment
            else trivia.indent
        )

    def _remove_empty_aot(self, path: PathParts, comment: str) -> None:
        parent = self._item(path[:-1])
        container = parent.value if isinstance(parent, AbstractTable) else parent
        if not isinstance(container, Container):
            raise ValueError("cannot safely detach this TOML array header comment")
        index = next(
            index
            for index, (key, _) in enumerate(container.body)
            if key is not None and key.key == path[-1]
        )
        del container[cast(str, path[-1])]
        if comment:
            container.body[index] = (None, tomlkit.ws(comment))

    def empty_syntax(self, path: PathParts) -> bytes:
        node = self._item(path)
        if isinstance(node, OutOfOrderTableProxy):
            raise ValueError("a noncontiguous TOML table cannot be pruned")
        raw = node.as_string()
        if isinstance(node, AbstractTable):
            raw += node.trivia.comment_ws + node.trivia.comment
        return raw.encode()
