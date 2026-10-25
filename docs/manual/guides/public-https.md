# A public HTTPS address

Holon's servers speak plain HTTP/1.1 and listen on 127.0.0.1. That is right for a machine of
your own, but a webhook sender like Stripe needs a public `https://` address. This guide puts
HTTPS in front, with a reverse proxy or a tunnel.

There are two things you might expose, and they are separate:

- **a deployed flow's port** (8103 for the Stripe flow): what callers and webhooks reach. Each
  server flow has its own port. This is what most people need.
- **the server and editor** (7420): only if people should use the editor from other machines.

## What to know first

- **TLS ends at the proxy.** Holon has no TLS of its own yet. Keep the proxy and Holon on the
  same machine, or on a private network you trust.
- **A flow's port has no sign-in.** Anyone who can reach it can call its routes. A flow guards
  itself: a verified signature (`stripe.verify`), or a token compared with `secure_equals` (the
  `guarded-api` sample). Everything else is answered as the flow says.
- **Limits on a flow's port**: a body of at most 10 MiB (413 past that), 10 seconds to send the
  headers and 30 for the body, and 1024 connections at once. There is no rate limit: put one in
  the proxy if you need it.
- **Forwarded headers** (`X-Forwarded-For` and the like) reach the flow as ordinary headers,
  `req.headers["x-forwarded-for"]`. Holon itself doesn't read them.

## A reverse proxy: Caddy

[Caddy](https://caddyserver.com) gets and renews certificates by itself. With a DNS name pointing
at the machine, a `Caddyfile` of:

```
hooks.example.com {
	reverse_proxy 127.0.0.1:8103
}
```

and `caddy run` in its folder, `https://hooks.example.com/stripe` reaches the Stripe flow. Any
proxy works the same way (nginx, Traefik, a cloud load balancer): forward HTTPS to the flow's port
on 127.0.0.1.

A flow's port accepts any `Host` header, so nothing in Holon needs to change.

Several flows can share one name, by path:

```
hooks.example.com {
	handle /stripe* {
		reverse_proxy 127.0.0.1:8103
	}
	handle /notes* {
		reverse_proxy 127.0.0.1:8102
	}
}
```

### The proxy on another machine

Flows listen on 127.0.0.1 unless the server is told otherwise. For a proxy on another machine of
your private network, start the server with the address its flows should listen on:

```sh
holon serve --data-host 10.0.0.5
```

(`0.0.0.0` listens on every interface; only do that behind a firewall.) The flows' ports then take
connections from the network, unencrypted, so keep them off the public internet and let only the
proxy reach them.

## A tunnel

For testing, or a machine with no public address, a tunnel gives a public HTTPS address with no
DNS or certificates:

```sh
cloudflared tunnel --url http://127.0.0.1:8103
```

prints an `https://….trycloudflare.com` address that reaches the flow. `ngrok http 8103` does the
same. A quick tunnel's address changes each time it starts, so update the webhook's endpoint
(and, for Stripe, its signing secret) when it does. For something lasting, use a named tunnel or
a proxy.

## Exposing the editor

To use the editor from other machines, proxy the server's own address:

```
holon.example.com {
	reverse_proxy 127.0.0.1:7420
}
```

and tell the server the name it is reached by, and the origin the editor's pages come from:

```sh
holon serve --allow-host holon.example.com --allow-origin https://holon.example.com
```

- `--allow-host`: the server answers only requests whose `Host` is localhost or a name it was
  given, which keeps other sites from reaching it through your browser. Give the name the proxy
  passes on (and `name:port` if the public port differs from the one it listens on).
- `--allow-origin`: the editor's own requests carry their page's origin, which must be one the
  server knows.

Everyone still signs in. Use long passwords, or an identity provider (the admin page), and make
API tokens with the narrowest role that works.

To reach the server from your network without a proxy, `--at 0.0.0.0:7420` listens on every
interface, still unencrypted: only on a network you trust.

## Checking it

```sh
curl -i https://hooks.example.com/stripe -X POST -d '{}'
```

should answer `400` from the Stripe flow (the request isn't signed), and appear as a run in the
editor's view mode. If the editor says **⚠ not answering** for the deployment, another program
holds its port: free the port, then deploy again.
