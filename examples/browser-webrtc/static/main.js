export function main(){
    let message = "main.js:main";
    console.log(message);
    let res = app("app("+message+")");
    console.log(res);
}
