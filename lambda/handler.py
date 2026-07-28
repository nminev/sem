import json
import os
import subprocess

SEM_PATH = "/var/task/sem"

RESPONSE_HEADERS = {"content-type": "application/json"}


def _respond(status_code, payload):
    return {
        "statusCode": status_code,
        "headers": RESPONSE_HEADERS,
        "body": json.dumps(payload),
    }


def lambda_handler(event, context):
    try:
        # Function URL / APIGW-v2 delivers the request payload as a JSON string
        # in event["body"]. Direct SDK invocation passes fields on event itself.
        raw_body = event.get("body")
        if isinstance(raw_body, str):
            payload = json.loads(raw_body) if raw_body else {}
        elif isinstance(raw_body, dict):
            payload = raw_body
        else:
            payload = event

        original = payload.get("original")
        modified = payload.get("modified")
        filename = payload.get("filename", "data.json")
        output_format = payload.get("format", "json")

        if original is None or modified is None:
            return _respond(400, {"error": "Both 'original' and 'modified' fields are required"})

        before_content = json.dumps(original, indent=2) if isinstance(original, (dict, list)) else str(original)
        after_content = json.dumps(modified, indent=2) if isinstance(modified, (dict, list)) else str(modified)

        file_changes = [
            {
                "filePath": filename,
                "status": "modified",
                "beforeContent": before_content,
                "afterContent": after_content,
            }
        ]

        result = subprocess.run(
            [SEM_PATH, "diff", "--stdin", "--format", output_format],
            input=json.dumps(file_changes),
            capture_output=True,
            text=True,
            timeout=30,
        )

        if result.returncode != 0:
            return _respond(500, {"error": result.stderr.strip()})

        if output_format == "json":
            try:
                return _respond(200, json.loads(result.stdout))
            except json.JSONDecodeError:
                return _respond(200, {"raw": result.stdout})
        return _respond(200, {"output": result.stdout})

    except json.JSONDecodeError as e:
        return _respond(400, {"error": f"invalid JSON in request body: {e}"})
    except subprocess.TimeoutExpired:
        return _respond(504, {"error": "sem timed out"})
    except Exception as e:
        return _respond(500, {"error": str(e)})
