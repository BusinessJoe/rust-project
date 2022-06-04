// The worker has its own scope and no direct access to functions/objects of the
// global scope. We import the generated JS file to make `wasm_bindgen`
// available which we need to initialize our WASM code.
importScripts("./pkg/wallpaper_evolution.js");

console.log("Initializing worker")

// In the worker, we have a different struct that we want to use as in
// `index.js`.
//const {NumberEval} = wasm_bindgen;
const { TestStruct, JsRandomCircle } = wasm_bindgen;


let test_struct;

async function init_wasm_in_worker() {
    // Load the wasm file by awaiting the Promise returned by `wasm_bindgen`.
    await wasm_bindgen("./pkg/wallpaper_evolution_bg.wasm");

    console.log("among");

    // Set callback to handle messages passed to the worker.
    self.onmessage = async event => {
        const { type, payload } = event.data;
        console.log("processing event", type);

        switch (type) {
            case "init/url":
                const url = payload;
                await TestStruct.new_async(url)
                    .then(struct => {
                        console.log("loaded async!");
                        test_struct = struct;
                        self.postMessage({
                            type: "init/done",
                            payload: [test_struct.get_target_width(), test_struct.get_target_height()]
                        });
                    })
                    .catch(err => {
                        console.error(err);
                        self.postMessage({ type: "init/error" });
                    });
                break;
            case "epoch":
                const { gen_size, num_gens } = payload;
                let best_circle = test_struct.try_epoch(gen_size, num_gens);

                if (best_circle) {
                    // Send response back to be handled by callback in main thread.
                    self.postMessage({
                        type: "epoch/done",
                        payload: {
                            imgx: best_circle.imgx,
                            imgy: best_circle.imgy,
                            center_x: best_circle.center_x,
                            center_y: best_circle.center_y,
                            radius: best_circle.radius,
                            color: best_circle.color,
                        }
                    });
                } else {
                    self.postMessage({
                        type: "epoch/done",
                        payload: undefined
                    });
                }
                break;
            default:
                console.error(`action type '${type}' not recognized`);
                break;
        }
    };

    self.postMessage({ type: "ready" });
};

init_wasm_in_worker();