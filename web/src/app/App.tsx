import { BrowserRouter } from "react-router";

import { Providers } from "./providers";
import { AppRouter } from "./router";

export function App() {
  return (
    <Providers>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </Providers>
  );
}
