import sys
import json
import subprocess

def log_debug(msg):
    sys.stderr.write(f"DEBUG: {msg}\n")
    sys.stderr.flush()

def handle_initialize(req_id, params):
    response = {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "tshark-mcp",
                "version": "1.0.0"
            }
        }
    }
    return response

def handle_tools_list(req_id):
    response = {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "tools": [
                {
                    "name": "sniff_packets",
                    "description": "Capture packets on a specified network interface using tshark.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "interface": {
                                "type": "string",
                                "description": "The network interface to sniff on (e.g. en2)."
                            },
                            "count": {
                                "type": "integer",
                                "description": "Number of packets to capture. Default is 10."
                            },
                            "filter": {
                                "type": "string",
                                "description": "Display filter for tshark (e.g. 'bootp' for DHCP, 'tcp.port == 80', 'arp')."
                            },
                            "verbose": {
                                "type": "boolean",
                                "description": "Whether to output detailed multi-line packet parsing (-V flag in tshark)."
                            },
                            "format": {
                                "type": "string",
                                "description": "Alternative output format for tshark (e.g. 'json', 'ps', 'text', 'fields')."
                            }
                        },
                        "required": ["interface"]
                    }
                }
            ]
        }
    }
    return response

def handle_tools_call(req_id, name, arguments):
    if name != "sniff_packets":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {"code": -32601, "message": f"Tool not found: {name}"}
        }

    interface = arguments.get("interface")
    count = arguments.get("count", 10)
    display_filter = arguments.get("filter")
    verbose = arguments.get("verbose", False)
    output_format = arguments.get("format")

    cmd = [
        "tshark",
        "-i", interface,
        "-c", str(count)
    ]

    if display_filter:
        cmd.extend(["-Y", display_filter])

    if verbose:
        cmd.append("-V")

    if output_format:
        cmd.extend(["-T", output_format])

    try:
        log_debug(f"Running command: {' '.join(cmd)}")
        # Run tshark with a timeout to avoid hanging forever if no packets arrive
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=15)
        
        output = res.stdout.strip()
        stderr = res.stderr.strip()
        
        if not output:
            if stderr:
                result_text = f"No packets captured.\nErrors/Warnings:\n{stderr}"
            else:
                result_text = "No packets captured (link might be inactive or no traffic)."
        else:
            result_text = output
            if stderr:
                result_text += f"\n\nWarnings/Errors:\n{stderr}"
                
    except subprocess.TimeoutExpired:
        result_text = "Capture timed out after 15 seconds (no packets matched the criteria)."
    except Exception as e:
        result_text = f"Failed to execute tshark: {str(e)}"

    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "content": [
                {
                    "type": "text",
                    "text": result_text
                }
            ]
        }
    }

def main():
    log_debug("tshark MCP server started")
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
