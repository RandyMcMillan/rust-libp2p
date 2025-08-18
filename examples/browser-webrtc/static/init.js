export function init(){
    let message = "init.js:init";
    console.log(message);
    let res = app("app("+message+")");
    console.log(res);
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export async function app(message) {
  console.log("app:start");
  await sleep(2000);
  console.log("after 2000ms");
  await sleep(1000);
  console.log("after 1000ms");
  console.log(""+message);
}
