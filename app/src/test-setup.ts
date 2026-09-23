// Browser APIs jsdom does not provide.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver

HTMLCanvasElement.prototype.getContext = function () {
  return new Proxy(
    {},
    {
      get: () => () => undefined,
      set: () => true,
    },
  )
} as unknown as typeof HTMLCanvasElement.prototype.getContext
