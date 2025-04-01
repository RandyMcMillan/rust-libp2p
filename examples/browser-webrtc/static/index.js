export function index(){
    let message = "index.js:index";
    console.log(message);
    let res = app("app("+message+")");
    console.log(res);
}
