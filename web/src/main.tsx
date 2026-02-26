import { render } from "solid-js/web";
import { App } from "./App";
import "./ui/theme/tokens.css";
import "./ui/theme/base.css";
import "./ui/primitives/index.css";
import "./ui/surfaces/error-screen.css";
import "./ui/surfaces/error-overlay.css";
import "./styles.css";

render(() => <App />, document.getElementById("root")!);
