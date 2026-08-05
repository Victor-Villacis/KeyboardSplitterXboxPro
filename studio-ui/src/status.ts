import { mount } from "@getforma/core";
import { StatusPage } from "./StatusPage";

// The mount() call is what the @getforma/compiler entry analyzer looks for to
// find the page component. The skeleton ships no client JS (build.mjs strips
// it from the manifest), so this file only ever runs at IR-emission time.
mount(() => StatusPage(), "#app");
