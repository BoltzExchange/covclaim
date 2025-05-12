# CovClaim

CovClaim is a daemon that connects to Elements via RPC and ZMQ or an Esplora REST API and watches the Liquid sidechain for claimable
covenants of Boltz swaps and broadcasts them.

## Disclaimer

CovClaim does **not** allow Boltz Swap clients to trustlessly receive Lightning payments when offline.

While a swap can be restricted to a specific claiming address, potentially giving this impression, it is crucial to acknowledge that
the swap creator retains sole control over the actual claiming conditions. If the receiving client cannot validate the swap's covenant
setup before the funds are sent (e.g., because they are offline), they are inherently trusting the entity that created the swap to
configure it correctly. From a trust perspective, this is similar to providing an xpub or wallet descriptor as swap destination
directly to the swap creator.

Note: swap transactions need to be unblinded for covenants and therefore cannot leverage the privacy benefits of
[Confidential Transactions](https://glossary.blockstream.com/confidential-transactions/).

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
