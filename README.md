# Census
[![Build status](https://ci.appveyor.com/api/projects/status/5eb6wkt9jpr7qqaj?svg=true)](https://ci.appveyor.com/project/fulmicoton/census)

This crate makes it possible to create an inventory object that keeps track of
instances of a given type.

It is used in tantivy to get an accurate list of all of the files that are still in use
by the index, and avoid garbage collecting them.


This `TrackedObject<T>` instance include some reference counting logic to ensure that
the object is removed from the inventory once the last instance is dropped.


# Example

```rust



```