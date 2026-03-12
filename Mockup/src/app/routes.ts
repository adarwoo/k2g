import { createBrowserRouter } from "react-router";
import { Setup } from "./pages/Setup";
import { Generator } from "./pages/Generator";
import { Output } from "./pages/Output";

export const router = createBrowserRouter([
  {
    path: "/",
    Component: Setup,
  },
  {
    path: "/generator",
    Component: Generator,
  },
  {
    path: "/output",
    Component: Output,
  },
]);