using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Threading;
using System.Management.Automation;
using System.Management.Automation.Subsystem;
using System.Management.Automation.Subsystem.Prediction;

namespace HintShell.PowerShell
{
    public class HintShellPredictor : ICommandPredictor
    {
        private readonly Guid _id = new Guid("8e6a1c2b-3d4e-5f6a-7b8c-9d0e1f2a3b4c");
        private const string PipeName = "hintshell";
        private const int TimeoutMs = 100; // 100ms max for suggestions

        public Guid Id => _id;
        public string Name => "HintShell";
        public string Description => "🧠 Personal Command Intelligence Engine";

        /// <summary>
        /// Register the predictor with PowerShell subsystem. Call from PowerShell.
        /// </summary>
        public static void Register()
        {
            var predictor = new HintShellPredictor();
            SubsystemManager.RegisterSubsystem(SubsystemKind.CommandPredictor, predictor);
        }

        public static void Unregister()
        {
            var id = new Guid("8e6a1c2b-3d4e-5f6a-7b8c-9d0e1f2a3b4c");
            SubsystemManager.UnregisterSubsystem(SubsystemKind.CommandPredictor, id);
        }

        /// <summary>
        /// Called by PSReadLine when the user types. Returns suggestions for inline/list display.
        /// </summary>
        public SuggestionPackage GetSuggestion(
            PredictionClient client,
            PredictionContext context,
            CancellationToken cancellationToken)
        {
            string input = context.InputAst.Extent.Text;

            if (string.IsNullOrWhiteSpace(input) || input.Length < 2)
            {
                return default;
            }

            try
            {
                var suggestions = GetSuggestionsFromDaemon(input, 10, cancellationToken);

                if (suggestions == null || suggestions.Count == 0)
                {
                    return default;
                }

                var result = new List<PredictiveSuggestion>();
                foreach (var s in suggestions)
                {
                    // Tooltip hiển thị khi bấm Alt+F1
                    string tooltip = $"🔢 Used {s.Frequency}x  |  📊 Score: {s.Score:F1}  |  🧠 HintShell";
                    result.Add(new PredictiveSuggestion(s.Command, tooltip));
                }

                return new SuggestionPackage(result);
            }
            catch
            {
                return default;
            }
        }

        /// <summary>
        /// Called when a suggestion is accepted or dismissed. Used for feedback/learning.
        /// </summary>
        public void OnSuggestionAccepted(PredictionClient client, uint session, string acceptedSuggestion)
        {
            // No-op for now. Could be used to boost ranking of accepted suggestions.
        }

        /// <summary>
        /// Called when command is about to execute. We use this to record the command to history.
        /// </summary>
        public void OnCommandLineExecuted(PredictionClient client, string commandLine, bool success)
        {
            if (string.IsNullOrWhiteSpace(commandLine))
                return;

            try
            {
                AddCommandToDaemon(commandLine);
            }
            catch
            {
                // Silently ignore - don't break user's terminal
            }
        }

        public void OnCommandLineAccepted(PredictionClient client, IReadOnlyList<string> history)
        {
            // Record the last executed command
            if (history.Count > 0)
            {
                string lastCommand = history[history.Count - 1];
                if (!string.IsNullOrWhiteSpace(lastCommand))
                {
                    try { AddCommandToDaemon(lastCommand); } catch { }
                }
            }
        }

        public void OnSuggestionDisplayed(PredictionClient client, uint session, int countOrIndex) { }

        #region Named Pipe Communication

        private List<SuggestionItem>? GetSuggestionsFromDaemon(string input, int limit, CancellationToken ct)
        {
            var request = new HintShellRequest
            {
                Action = "suggest",
                Input = input,
                Limit = limit
            };

            string requestJson = JsonSerializer.Serialize(request) + "\n";
            string? responseJson = SendToPipe(requestJson, ct);

            if (string.IsNullOrEmpty(responseJson))
                return null;

            var response = JsonSerializer.Deserialize<HintShellResponse>(responseJson);
            return response?.Suggestions;
        }

        private void AddCommandToDaemon(string command)
        {
            var request = new AddCommandRequest
            {
                Action = "add",
                Command = command,
                Shell = "powershell"
            };

            string requestJson = JsonSerializer.Serialize(request) + "\n";

            using var cts = new CancellationTokenSource(500);
            SendToPipe(requestJson, cts.Token);
        }

        private string? SendToPipe(string message, CancellationToken ct)
        {
            try
            {
                using var pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.InOut);
                pipe.Connect(TimeoutMs);

                byte[] requestBytes = Encoding.UTF8.GetBytes(message);
                pipe.Write(requestBytes, 0, requestBytes.Length);
                pipe.Flush();

                using var reader = new StreamReader(pipe, Encoding.UTF8);
                string? response = reader.ReadLine();
                return response;
            }
            catch (TimeoutException)
            {
                return null;
            }
            catch (IOException)
            {
                return null;
            }
        }

        #endregion
    }

    #region JSON Models

    internal class HintShellRequest
    {
        [JsonPropertyName("action")]
        public string Action { get; set; } = "";

        [JsonPropertyName("input")]
        public string? Input { get; set; }

        [JsonPropertyName("limit")]
        public int Limit { get; set; } = 5;
    }

    internal class AddCommandRequest
    {
        [JsonPropertyName("action")]
        public string Action { get; set; } = "add";

        [JsonPropertyName("command")]
        public string Command { get; set; } = "";

        [JsonPropertyName("shell")]
        public string? Shell { get; set; }
    }

    internal class HintShellResponse
    {
        [JsonPropertyName("success")]
        public bool Success { get; set; }

        [JsonPropertyName("suggestions")]
        public List<SuggestionItem>? Suggestions { get; set; }
    }

    internal class SuggestionItem
    {
        [JsonPropertyName("command")]
        public string Command { get; set; } = "";

        [JsonPropertyName("score")]
        public double Score { get; set; }

        [JsonPropertyName("frequency")]
        public long Frequency { get; set; }
    }

    #endregion
}
