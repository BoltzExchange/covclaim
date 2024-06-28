# covclaim

This is a daemon that connects to Elements via RPC and ZMQ or an Esplora REST API and watches the chain for claimable
covenants of Boltz swaps and broadcasts them.

## Building

The latest stable version of Rust is required to build covclaim.

```bash
cargo build --release
```

## Configuration

The configuration of covclaim is in the `.env` file.

## REST API

To register a new reverse swap the daemon should watch for:

`POST /covenant`

With these values in the request body:

```JSON
{
  "claimPublicKey": "<public key of the user>",
  "refundPublicKey": "<public key of Boltz>",
  "preimage": "<preimage of the swap>",
  "blindingKey": "<blinding key of the lockup address of the swap>",
  "address": "<address to which the covenant should be claimed>",
  "tree": "<the swapTree of the response when creating the swap as object>"
}
```
