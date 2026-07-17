import sys
import json
import glob
import time

# Attempt to import serial library
try:
    import serial
    import serial.tools.list_ports
    HAS_SERIAL = True
except ImportError:
    HAS_SERIAL = False

def log_debug(msg):
    sys.stderr.write(f"DEBUG: {msg}\n")
    sys.stderr.flush()

def handle_initialize(req_id, params):
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "serial-mcp",
                "version": "1.0.0"
            }
        }
    }

def handle_tools_list(req_id):
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "tools": [
                {
                    "name": "list_serial_ports",
                    "description": "List all available serial / USB-modem ports on macOS.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "read_serial_data",
                    "description": "Read incoming log data from a serial port.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "port": {
                                "type": "string",
                                "description": "The serial port path (e.g. /dev/cu.usbmodem123456801)."
                            },
                            "baudrate": {
                                "type": "integer",
                                "description": "Baudrate for connection. Default is 115200."
                            },
                            "timeout_seconds": {
                                "type": "integer",
                                "description": "Duration to listen in seconds. Default is 5."
                            }
                        },
                        "required": ["port"]
                    }
                }
            ]
        }
    }

def handle_tools_call(req_id, name, arguments):
    if not HAS_SERIAL:
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "content": [
                    {
                        "type": "text",
                        "text": "Error: 'pyserial' package is not installed. Please run 'pip3 install pyserial' in your terminal to enable this tool."
                    }
                ]
            }
        }

    if name == "list_serial_ports":
        ports = glob.glob("/dev/cu.usbmodem*") + glob.glob("/dev/cu.usbserial*")
        if not ports:
            result_text = "No USB serial or modem ports found in /dev/cu.*"
        else:
            result_text = "Available ports:\n" + "\n".join(ports)
            
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "content": [{"type": "text", "text": result_text}]
            }
        }

    elif name == "read_serial_data":
        port = arguments.get("port")
        baudrate = arguments.get("baudrate", 115200)
        timeout = arguments.get("timeout_seconds", 5)

        try:
            log_debug(f"Opening port {port} at {baudrate} baud...")
            ser = serial.Serial(port, baudrate, timeout=1.0)
            
            log_debug(f"Listening for {timeout} seconds...")
            start_time = time.time()
            received_bytes = bytearray()
            
            while time.time() - start_time < timeout:
                data = ser.read(100)
                if data:
                    received_bytes.extend(data)
                time.sleep(0.05)
                
            ser.close()
            
            output_text = received_bytes.decode('utf-8', errors='replace')
            if not output_text:
                result_text = "Connected successfully, but no data received."
            else:
                result_text = f"--- Serial Data Received ---\n{output_text}"
        except Exception as e:
            result_text = f"Failed to read from serial port: {str(e)}"

        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "content": [{"type": "text", "text": result_text}]
            }
        }

    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "error": {"code": -32601, "message": f"Tool not found: {name}"}
    }

def main():
    log_debug("serial MCP server started")
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        if not line.strip():
            continue
        try:
            req = json.loads(line)
            req_id = req.get("id")
            method = req.get("method")
            
            log_debug(f"Received request: {method} (id: {req_id})")
            
            if req_id is None:
                # This is a notification, do not send a response
                continue
            
            if method == "initialize":
                resp = handle_initialize(req_id, req.get("params", {}))
            elif method == "tools/list":
                resp = handle_tools_list(req_id)
            elif method == "tools/call":
                params = req.get("params", {})
                resp = handle_tools_call(req_id, params.get("name"), params.get("arguments", {}))
            else:
                resp = {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": {"code": -32601, "message": f"Method not found: {method}"}
                }
            
            sys.stdout.write(json.dumps(resp) + "\n")
            sys.stdout.flush()
        except Exception as e:
            log_debug(f"Error handling request: {str(e)}")

if __name__ == "__main__":
    main()
