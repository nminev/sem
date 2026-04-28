import json
import os
import subprocess

SEM_PATH = "/var/task/sem"


def lambda_handler(event, context):
    try:
        original = event.get("original")
        modified = event.get("modified")
        filename = event.get("filename", "data.json")
        output_format = event.get("format", "json")

        if original is None or modified is None:
            return {
                "statusCode": 400,
                "body": {"error": "Both 'original' and 'modified' fields are required"}
            }

        # Accept dicts/lists (parsed JSON) or raw strings
        before_content = json.dumps(original, indent=2) if isinstance(original, (dict, list)) else str(original)
        after_content = json.dumps(modified, indent=2) if isinstance(modified, (dict, list)) else str(modified)

        file_changes = [
            {
                "filePath": filename,
                "status": "modified",
                "beforeContent": before_content,
                "afterContent": after_content
            }
        ]

        result = subprocess.run(
            [SEM_PATH, "diff", "--stdin", "--format", output_format],
            input=json.dumps(file_changes),
            capture_output=True,
            text=True,
            timeout=30
        )

        if result.returncode != 0:
            return {
                "statusCode": 500,
                "body": {"error": result.stderr.strip()}
            }

        if output_format == "json":
            try:
                diff_result = json.loads(result.stdout)
            except json.JSONDecodeError:
                diff_result = result.stdout
        else:
            diff_result = result.stdout

        return {
            "statusCode": 200,
            "body": diff_result
        }

    except subprocess.TimeoutExpired:
        return {"statusCode": 504, "body": {"error": "sem timed out"}}
    except Exception as e:
        return {"statusCode": 500, "body": {"error": str(e)}}
