import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";

import "./styles.css";
import { getRouter } from "./router";

const router = getRouter();

const root = document.getElementById("root")!;

createRoot(root).render(
  <RouterProvider router={router} />,
);
