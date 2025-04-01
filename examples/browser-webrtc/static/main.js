// Example 1: Generate a random number between a given range
function getRandomNumber(min, max) {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

// Example 2: Check if a string is a palindrome
function isPalindrome(str) {
  const cleanStr = str.toLowerCase().replace(/[^a-z0-9]/g, '');
  const reversedStr = cleanStr.split('').reverse().join('');
  return cleanStr === reversedStr;
}

// Example 3: Delay execution of an async function
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function init(){

let message = "init:Hello, world!";
console.log(message); // Output: Hello, world!

}
export function run(){

let message = "run:Hello, world!";
console.log(message); // Output: Hello, world!

}
