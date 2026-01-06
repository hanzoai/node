
            
            async function run(configurations, params) {
                return {
                    message: `Hello ${params.name}!`
                };
            }
        
            const configurations = JSON.parse('{}');
            const parameters = JSON.parse('{\"name\":\"World\"}');

            const result = await run(configurations, parameters);
            const adaptedResult = result === undefined ? null : result;
            console.log("<hanzo-code-result>");
            console.log(JSON.stringify(adaptedResult));
            console.log("</hanzo-code-result>");
            Deno.exit(0);
        