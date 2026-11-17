// What failed to draw says so, in its place, and the rest of the console goes on (R44). A page, or
// the canvas, that throws while it renders took the whole editor down to a blank page: the open
// flows' unsaved changes with it, since they live in the editor around it. `PageBoundary` holds
// one part: its error is said where it was, with a way to try it again, and it is tried again by
// itself once `resetKey` changes (another page opened). `RootBoundary` holds what is left (the
// sign-in, the editor's own state): only a reload goes on from there, so it says so.

import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = { children: ReactNode };
type State = { error: Error | null };

/** What threw, as a person reads it: its message, and where (the component stack) to report. */
function Said({ error, stack }: { error: Error; stack: string }) {
  return (
    <>
      <p className="hl-crash__message">{error.message || String(error)}</p>
      {stack && (
        <details className="hl-crash__where">
          <summary>Where it failed</summary>
          <pre>{stack.trim()}</pre>
        </details>
      )}
    </>
  );
}

/** One part of the console: `what` it is ("This page", "The canvas"), said in its error. */
export class PageBoundary extends Component<Props & { what: string; resetKey?: string }, State & { stack: string; key?: string }> {
  state: State & { stack: string; key?: string } = { error: null, stack: "", key: this.props.resetKey };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  static getDerivedStateFromProps(props: { resetKey?: string }, state: State & { key?: string }) {
    // Another page opened: what failed is tried again.
    return props.resetKey === state.key ? null : { error: null, stack: "", key: props.resetKey };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: info.componentStack ?? "" });
    console.error(`${this.props.what} failed:`, error);
  }

  render() {
    const { error, stack } = this.state;
    if (!error) return this.props.children;
    return (
      <div role="alert" className="hl-crash">
        <h1 className="hl-crash__head">{this.props.what} failed</h1>
        <Said error={error} stack={stack} />
        <p className="hl-flow__note">The rest of the console goes on: your open flows, and what isn&apos;t saved in them, are kept.</p>
        <button className="hl-flow__button" onClick={() => this.setState({ error: null, stack: "" })}>
          Try again
        </button>
      </div>
    );
  }
}

/** The whole console: what no part's boundary held. */
export class RootBoundary extends Component<Props, State & { stack: string }> {
  state: State & { stack: string } = { error: null, stack: "" };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: info.componentStack ?? "" });
    console.error("The console failed:", error);
  }

  render() {
    const { error, stack } = this.state;
    if (!error) return this.props.children;
    return (
      <div role="alert" className="hl-crash hl-crash--root">
        <h1 className="hl-crash__head">The console failed</h1>
        <Said error={error} stack={stack} />
        <p className="hl-flow__note">Reloading the page goes on from here; changes not saved before this are lost.</p>
        <button className="hl-flow__button" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    );
  }
}
