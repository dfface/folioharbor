import { render } from "@testing-library/react";

import { App } from "../app/App";

export function renderApp(route: string) {
  window.history.pushState({}, "", route);
  return render(<App />);
}
