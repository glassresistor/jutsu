/* @ts-self-types="./jutsu_wasm.d.ts" */
import * as wasm from "./jutsu_wasm_bg.wasm";
import { __wbg_set_wasm } from "./jutsu_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export {
    last_error, tokenize, tokenize_custom
} from "./jutsu_wasm_bg.js";
