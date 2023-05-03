# Noria MongoDB adapter

This crate contains the MongoDB protocol adapter for
[Noria](https://github.com/mit-pdos/noria). The adapter allows legacy
applications that use ~~parameterized MySQL queries~~ MongoDB MQL to directly start
using Noria, with no or minimal source code changes.

## Running the adapter
To run the adapter and listen on the default MongoDB port (27017), simply type:

```console
$ cargo run --release -- --deployment $DEPLOYMENT
```
`DEPLOYMENT` is the same deployment ID you used when starting
the Noria server.

If you would like to use a different port (e.g., because you're also running
a MongoDB server), pass `-a <IP>:<PORT>` making sure to specify the desired bind
ip as well.

## Connecting to Noria
The MongoDB adapter uses ZooKeeper to find the Noria server. To specify the
ZooKeeper server location, pass the `-z` argument:

```console
$ cargo run --release -- --deployment $DEPLOYMENT --authority-address 172.16.0.19:2181
```
... for a ZooKeeper server listening on port `2181` at IP `172.16.0.19`.
