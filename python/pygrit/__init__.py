"""pygrit — Python bindings for grit-lib."""

import enum

from pygrit._pygrit import (
    GritError,
    InvalidObjectError,
    ObjectId,
    ObjectNotFoundError,
    RepositoryError,
    _discover_head_hex,
    _hello,
)
from pygrit._pygrit import ObjectKind as _NativeObjectKind

# AIDEV-NOTE: PyO3 0.23 native enums are NOT Python enum.Enum subclasses — they are
# not iterable at the type level and members lack a `.name` attribute. The public
# pygrit.ObjectKind is therefore a thin enum.IntEnum facade (design: "thin Python
# facade, not a literal 1:1 re-export"). Its integer values are taken from the native
# `#[pyclass(eq_int)]` discriminants so the two interoperate by int-equality when a
# later odb/parse binding (task 2.6+) surfaces the native kind to Python. Keep these
# in sync with src/objects.rs: variant declaration order defines the discriminant.
_NATIVE_KIND_VALUES = {
    "COMMIT": 0,
    "TREE": 1,
    "BLOB": 2,
    "TAG": 3,
}


class ObjectKind(enum.IntEnum):
    """Git object kind (blob, tree, commit, tag)."""

    COMMIT = _NATIVE_KIND_VALUES["COMMIT"]
    TREE = _NATIVE_KIND_VALUES["TREE"]
    BLOB = _NATIVE_KIND_VALUES["BLOB"]
    TAG = _NATIVE_KIND_VALUES["TAG"]


# Guard against the Rust eq_int discriminants drifting from the facade values above;
# this also keeps the native import live for round-trip use by later bindings.
for _name, _value in _NATIVE_KIND_VALUES.items():
    if getattr(_NativeObjectKind, _name) != _value:  # pragma: no cover
        raise RuntimeError(
            f"native ObjectKind.{_name} discriminant drifted from facade value {_value}"
        )
del _name, _value


__all__ = [
    "GritError",
    "InvalidObjectError",
    "ObjectId",
    "ObjectKind",
    "ObjectNotFoundError",
    "RepositoryError",
    "_discover_head_hex",
    "_hello",
]
