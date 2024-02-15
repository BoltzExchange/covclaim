# covclaim

This is a daemon that connects to Elements via RPC and ZMQ and watches the chain for claimable covenants of Boltz reverse swaps and broadcasts them.

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
