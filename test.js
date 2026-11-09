

//Simple terminal output
// console.log("Hello world");


//Simple server setup so that we can run code in the browser. type in 127.0.0.1:3000 to see the output "Hello world"

const http = require('http');

const hostname = '127.0.0.1';
const port = 3000;

const server = http.createServer((req, res) =>{
    res.statusCode = 200;
    res.setHeader('Content-Type', 'text/plain');
    res.end('Hello world');

});

server.listen(port, hostname, () => {
    console.log('Server running at http://${hostname}:${port}/');
});