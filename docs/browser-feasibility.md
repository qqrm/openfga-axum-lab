# Browser deployment feasibility

- GitHub Pages serves a static site; ordinary Pages from a public repository
  is publicly reachable. It does not provide Docker Engine or server-side
  PostgreSQL/OpenFGA.
- [container2wasm](https://github.com/container2wasm/container2wasm) converts
  Linux containers into an emulated WASM environment. Its browser mode needs
  special networking and isolation headers; its ability to run this particular
  Axum + OpenFGA + PostgreSQL stack and expose the application UI is unproven.
  The project's [GitHub Pages demo](https://ktock.github.io/container2wasm-demo/)
  lists downloads of approximately 78–200 MB for individual example images;
  these are the demo author's figures, not measurements of this application.
- A local attempt to convert even Alpine was blocked by package-network timeouts
  inside Docker build steps. The manual GitHub workflow is a reproducible,
  bounded feasibility experiment; Alpine conversion alone cannot establish that
  our full stack works.
- [Webernetes](https://github.com/ngrok/webernetes) simulates a subset of
  Kubernetes in a browser but does not run ordinary OCI images.
  [PGlite](https://pglite.dev/docs/about) runs PostgreSQL as WASM, but is not
  equivalent to running the whole application and OpenFGA unchanged.

Do not label a simulated UI or a single converted test container as the real
end-to-end service. A public deployment requires a separate verified privacy
audit and working, explicitly identified backend or browser runtime.
